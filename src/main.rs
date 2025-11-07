use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};
use std::io;
use tokio::runtime::Runtime;

// Clash API 数据结构
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ProxyInfo {
    name: String,
    #[serde(rename = "type")]
    proxy_type: String,
    now: Option<String>,
    all: Option<Vec<String>>,
    history: Option<Vec<DelayHistory>>,
    udp: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DelayHistory {
    time: String,
    delay: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ProxiesResponse {
    proxies: std::collections::HashMap<String, ProxyInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Rule {
    #[serde(rename = "type")]
    rule_type: String,
    payload: String,
    proxy: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RulesResponse {
    rules: Vec<Rule>,
}

// 应用状态
struct App {
    proxy_groups: Vec<ProxyGroup>,
    selected_group_index: usize,
    selected_proxy_index: usize,
    rules: Vec<Rule>,
    selected_rule_index: usize,
    rule_filter: String,
    editing_rule_filter: bool,
    focus: Focus,
    api_url: String,
    secret: Option<String>,
}

#[derive(Clone)]
struct ProxyGroup {
    name: String,
    current: Option<String>,
    proxies: Vec<ProxyItem>,
}

#[derive(Clone)]
struct ProxyItem {
    name: String,
    delay: Option<u32>,
}

#[derive(PartialEq)]
enum Focus {
    GroupList,
    ProxyList,
    RuleList,
}

impl App {
    fn new(api_url: String, secret: Option<String>) -> Self {
        Self {
            proxy_groups: Vec::new(),
            selected_group_index: 0,
            selected_proxy_index: 0,
            rules: Vec::new(),
            selected_rule_index: 0,
            rule_filter: String::new(),
            editing_rule_filter: false,
            focus: Focus::GroupList,
            api_url,
            secret,
        }
    }

    async fn fetch_proxies(&mut self) -> Result<()> {
        // 保存当前选中的代理组名称
        let selected_group_name = self
            .proxy_groups
            .get(self.selected_group_index)
            .map(|g| g.name.clone());

        let client = reqwest::Client::new();
        let url = format!("{}/proxies", self.api_url);

        let mut request = client.get(&url);
        if let Some(secret) = &self.secret {
            request = request.header("Authorization", format!("Bearer {}", secret));
        }

        let response: ProxiesResponse = request.send().await?.json().await?;

        self.proxy_groups.clear();

        for (name, info) in response.proxies.iter() {
            // 只显示代理组（有 all 字段的）
            if let Some(all) = &info.all {
                let mut proxies = Vec::new();
                for proxy_name in all {
                    let delay = response.proxies.get(proxy_name)
                        .and_then(|p| p.history.as_ref())
                        .and_then(|h| h.last())
                        .map(|d| d.delay);

                    proxies.push(ProxyItem {
                        name: proxy_name.clone(),
                        delay,
                    });
                }

                self.proxy_groups.push(ProxyGroup {
                    name: name.clone(),
                    current: info.now.clone(),
                    proxies,
                });
            }
        }

        // 按名称排序
        self.proxy_groups.sort_by(|a, b| a.name.cmp(&b.name));

        // 恢复之前选中的代理组位置
        if let Some(name) = selected_group_name {
            if let Some(pos) = self.proxy_groups.iter().position(|g| g.name == name) {
                self.selected_group_index = pos;
            } else {
                self.selected_group_index = 0;
            }
        } else {
            self.selected_group_index = 0;
        }

        // 确保索引有效
        if self.proxy_groups.is_empty() {
            self.selected_group_index = 0;
            self.selected_proxy_index = 0;
        } else {
            if self.selected_group_index >= self.proxy_groups.len() {
                self.selected_group_index = 0;
            }
            // 自动选中当前正在使用的代理
            self.sync_selected_proxy();
        }

        Ok(())
    }

    async fn fetch_rules(&mut self) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/rules", self.api_url);

        let mut request = client.get(&url);
        if let Some(secret) = &self.secret {
            request = request.header("Authorization", format!("Bearer {}", secret));
        }

        let response: RulesResponse = request.send().await?.json().await?;
        self.rules = response.rules;

        // 确保索引有效
        self.ensure_rule_selection();

        Ok(())
    }

    async fn select_proxy(&self, group_name: &str, proxy_name: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/proxies/{}", self.api_url, group_name);

        let mut request = client.put(&url);
        if let Some(secret) = &self.secret {
            request = request.header("Authorization", format!("Bearer {}", secret));
        }

        let body = serde_json::json!({ "name": proxy_name });
        request.json(&body).send().await?;

        Ok(())
    }

    fn next_group(&mut self) {
        if !self.proxy_groups.is_empty() {
            self.selected_group_index = (self.selected_group_index + 1) % self.proxy_groups.len();
            self.sync_selected_proxy();
        }
    }

    fn previous_group(&mut self) {
        if !self.proxy_groups.is_empty() {
            if self.selected_group_index > 0 {
                self.selected_group_index -= 1;
            } else {
                self.selected_group_index = self.proxy_groups.len() - 1;
            }
            self.sync_selected_proxy();
        }
    }

    fn next_proxy(&mut self) {
        if let Some(group) = self.proxy_groups.get(self.selected_group_index) {
            if !group.proxies.is_empty() {
                self.selected_proxy_index = (self.selected_proxy_index + 1) % group.proxies.len();
            }
        }
    }

    fn previous_proxy(&mut self) {
        if let Some(group) = self.proxy_groups.get(self.selected_group_index) {
            if !group.proxies.is_empty() {
                if self.selected_proxy_index > 0 {
                    self.selected_proxy_index -= 1;
                } else {
                    self.selected_proxy_index = group.proxies.len() - 1;
                }
            }
        }
    }

    fn next_rule(&mut self) {
        let filtered = self.filtered_rule_indices();
        if filtered.is_empty() {
            return;
        }
        let current_pos = filtered
            .iter()
            .position(|&idx| idx == self.selected_rule_index)
            .unwrap_or(0);
        let next_pos = (current_pos + 1) % filtered.len();
        self.selected_rule_index = filtered[next_pos];
    }

    fn previous_rule(&mut self) {
        let filtered = self.filtered_rule_indices();
        if filtered.is_empty() {
            return;
        }
        let current_pos = filtered
            .iter()
            .position(|&idx| idx == self.selected_rule_index)
            .unwrap_or(filtered.len() - 1);
        let prev_pos = if current_pos == 0 {
            filtered.len() - 1
        } else {
            current_pos - 1
        };
        self.selected_rule_index = filtered[prev_pos];
    }

    fn toggle_focus(&mut self) {
        if self.editing_rule_filter {
            return;
        }
        self.focus = match self.focus {
            Focus::GroupList => Focus::ProxyList,
            Focus::ProxyList => Focus::RuleList,
            Focus::RuleList => Focus::GroupList,
        };
    }

    fn filtered_rule_indices(&self) -> Vec<usize> {
        if self.rule_filter.is_empty() {
            return (0..self.rules.len()).collect();
        }
        let filter = self.rule_filter.to_lowercase();
        self.rules
            .iter()
            .enumerate()
            .filter_map(|(idx, rule)| {
                let haystack = format!("{} {} {}", rule.rule_type, rule.payload, rule.proxy).to_lowercase();
                if haystack.contains(&filter) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    fn ensure_rule_selection(&mut self) {
        if self.rules.is_empty() {
            self.selected_rule_index = 0;
            return;
        }
        let filtered = self.filtered_rule_indices();
        if filtered.is_empty() {
            self.selected_rule_index = 0;
        } else if !filtered.contains(&self.selected_rule_index) {
            self.selected_rule_index = filtered[0];
        }
    }

    fn start_rule_filter_edit(&mut self) {
        self.focus = Focus::RuleList;
        self.editing_rule_filter = true;
    }

    fn apply_rule_filter_change(&mut self) {
        self.ensure_rule_selection();
    }

    fn clear_rule_filter(&mut self) {
        self.rule_filter.clear();
        self.ensure_rule_selection();
    }

    // 同步选中的代理索引到当前正在使用的代理
    fn sync_selected_proxy(&mut self) {
        if let Some(group) = self.proxy_groups.get(self.selected_group_index) {
            if let Some(current) = &group.current {
                // 查找当前使用的代理在列表中的位置
                if let Some(pos) = group.proxies.iter().position(|p| &p.name == current) {
                    self.selected_proxy_index = pos;
                    return;
                }
            }
        }
        // 如果找不到或没有当前代理，重置为 0
        self.selected_proxy_index = 0;
    }
}

fn main() -> Result<()> {
    // 创建 tokio runtime
    let rt = Runtime::new()?;

    // 设置终端
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 从环境变量或默认值获取配置
    let api_url = std::env::var("CLASH_API_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9091".to_string());
    let secret = std::env::var("CLASH_SECRET").ok();

    let mut app = App::new(api_url, secret);

    // 初始加载数据
    rt.block_on(async {
        if let Err(e) = app.fetch_proxies().await {
            eprintln!("Error fetching proxies: {}", e);
        }
        if let Err(e) = app.fetch_rules().await {
            eprintln!("Error fetching rules: {}", e);
        }
    });

    let res = rt.block_on(run_app(&mut terminal, &mut app, &rt));

    // 恢复终端
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {:?}", err);
    }

    Ok(())
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    _rt: &Runtime,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.editing_rule_filter {
                        match key.code {
                            KeyCode::Esc => {
                                app.editing_rule_filter = false;
                                app.apply_rule_filter_change();
                            }
                            KeyCode::Enter => {
                                app.editing_rule_filter = false;
                                app.apply_rule_filter_change();
                            }
                            KeyCode::Backspace => {
                                app.rule_filter.pop();
                                app.apply_rule_filter_change();
                            }
                            KeyCode::Delete => {
                                app.clear_rule_filter();
                            }
                            KeyCode::Char(ch) => {
                                app.rule_filter.push(ch);
                                app.apply_rule_filter_change();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('r') => {
                            // 刷新数据
                            app.fetch_proxies().await?;
                            app.fetch_rules().await?;
                        }
                        KeyCode::Tab => {
                            app.toggle_focus();
                        }
                        KeyCode::Char('/') => {
                            app.start_rule_filter_edit();
                        }
                        KeyCode::Char('c') => {
                            if app.focus == Focus::RuleList {
                                app.clear_rule_filter();
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => match app.focus {
                            Focus::GroupList => app.previous_group(),
                            Focus::ProxyList => app.previous_proxy(),
                            Focus::RuleList => app.previous_rule(),
                        },
                        KeyCode::Char('j') | KeyCode::Down => match app.focus {
                            Focus::GroupList => app.next_group(),
                            Focus::ProxyList => app.next_proxy(),
                            Focus::RuleList => app.next_rule(),
                        },
                        KeyCode::Enter => {
                            if app.focus == Focus::ProxyList {
                                if let Some(group) = app.proxy_groups.get(app.selected_group_index) {
                                    if let Some(proxy) = group.proxies.get(app.selected_proxy_index) {
                                        let group_name = group.name.clone();
                                        let proxy_name = proxy.name.clone();

                                        if let Err(e) = app.select_proxy(&group_name, &proxy_name).await {
                                            eprintln!("Error selecting proxy: {}", e);
                                        } else {
                                            // 更新成功后刷新数据
                                            app.fetch_proxies().await?;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // 标题
    let title = Paragraph::new("Clash TUI - 代理管理")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // 主内容区域 - 分成左右两部分
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[1]);

    // 左侧再分成上下两部分：代理组和代理列表
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_chunks[0]);

    // 代理组列表
    render_group_list(f, app, left_chunks[0]);

    // 代理列表
    render_proxy_list(f, app, left_chunks[1]);

    // 右侧显示规则列表
    render_rule_list(f, app, main_chunks[1]);

    // 帮助信息
    let help_text = vec![
        Span::raw("快捷键: "),
        Span::styled("Tab", Style::default().fg(Color::Yellow)),
        Span::raw(" 切换焦点 | "),
        Span::styled("↑↓/jk", Style::default().fg(Color::Yellow)),
        Span::raw(" 移动 | "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" 选择 | "),
        Span::styled("r", Style::default().fg(Color::Yellow)),
        Span::raw(" 刷新 | "),
        Span::styled("/", Style::default().fg(Color::Yellow)),
        Span::raw(" 规则过滤 | "),
        Span::styled("c", Style::default().fg(Color::Yellow)),
        Span::raw(" 清除过滤 | "),
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::raw(" 退出"),
    ];
    let help = Paragraph::new(Line::from(help_text))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

fn render_group_list(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .proxy_groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let current = group.current.as_deref().unwrap_or("未选择");
            let content = format!("{} → {}", group.name, current);
            let style = if i == app.selected_group_index && app.focus == Focus::GroupList {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if i == app.selected_group_index {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            ListItem::new(content).style(style)
        })
        .collect();

    let border_style = if app.focus == Focus::GroupList {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("代理组")
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow),
        );

    let mut state = ListState::default();
    state.select(Some(app.selected_group_index));

    f.render_stateful_widget(list, area, &mut state);
}

fn render_proxy_list(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = if let Some(group) = app.proxy_groups.get(app.selected_group_index) {
        group
            .proxies
            .iter()
            .enumerate()
            .map(|(i, proxy)| {
                let delay_str = match proxy.delay {
                    Some(delay) => format!("{} ms", delay),
                    None => "N/A".to_string(),
                };
                let is_current = group.current.as_ref() == Some(&proxy.name);
                let prefix = if is_current { "✓ " } else { "  " };
                let content = format!("{}{} ({})", prefix, proxy.name, delay_str);

                let style = if i == app.selected_proxy_index && app.focus == Focus::ProxyList {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if i == app.selected_proxy_index {
                    Style::default().fg(Color::Green)
                } else if is_current {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };

                ListItem::new(content).style(style)
            })
            .collect()
    } else {
        vec![]
    };

    let border_style = if app.focus == Focus::ProxyList {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let title = if let Some(group) = app.proxy_groups.get(app.selected_group_index) {
        format!("代理 - {}", group.name)
    } else {
        "代理".to_string()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow),
        );

    let mut state = ListState::default();
    state.select(Some(app.selected_proxy_index));

    f.render_stateful_widget(list, area, &mut state);
}

fn render_rule_list(f: &mut Frame, app: &App, area: Rect) {
    let filtered_indices = app.filtered_rule_indices();

    let items: Vec<ListItem> = filtered_indices
        .iter()
        .map(|&idx| {
            let rule = &app.rules[idx];
            let content = format!("{}: {} → {}", rule.rule_type, rule.payload, rule.proxy);
            let is_selected = app.selected_rule_index == idx;
            let style = if is_selected && app.focus == Focus::RuleList {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(if app.editing_rule_filter { Modifier::BOLD | Modifier::UNDERLINED } else { Modifier::BOLD })
            } else if is_selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            ListItem::new(content).style(style)
        })
        .collect();

    let border_style = if app.focus == Focus::RuleList {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let title = if app.rule_filter.is_empty() {
        format!("规则 ({}/{})", filtered_indices.len(), app.rules.len())
    } else {
        format!(
            "规则 ({}/{}) | 过滤: {}{}",
            filtered_indices.len(),
            app.rules.len(),
            app.rule_filter,
            if app.editing_rule_filter { " (编辑)" } else { "" }
        )
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow),
        );

    let mut state = ListState::default();
    let selected_position = filtered_indices
        .iter()
        .position(|&idx| idx == app.selected_rule_index);
    state.select(selected_position);

    f.render_stateful_widget(list, area, &mut state);
}
