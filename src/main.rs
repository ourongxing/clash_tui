use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};
use tokio::{runtime::Runtime, sync::mpsc, task::JoinSet};

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

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DelayResponse {
    delay: u32,
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
    active_tab: ActiveTab,
    focus: Focus,
    api_url: String,
    secret: Option<String>,
    delay_refreshing: bool,
    pinned_group_names: HashSet<String>,
    pending_g: bool,
    config_path: PathBuf,
    show_help: bool,
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

struct DelayRefreshRequest {
    group_name: String,
    proxy_names: Vec<String>,
    api_url: String,
    secret: Option<String>,
}

struct DelayRefreshResult {
    group_name: String,
    delays: Vec<(String, u32)>,
}

#[derive(Default, Deserialize, Serialize)]
struct AppConfig {
    pinned_group_names: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    GroupList,
    ProxyList,
    RuleList,
}

#[derive(Clone, Copy, PartialEq)]
enum ActiveTab {
    Proxy,
    Rules,
    About,
}

fn config_path() -> PathBuf {
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(config_home).join("clash_tui/config.json");
    }

    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/clash_tui/config.json");
    }

    PathBuf::from("clash_tui_config.json")
}

fn load_config(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn save_config(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content)?;

    Ok(())
}

impl App {
    fn new(api_url: String, secret: Option<String>) -> Self {
        let config_path = config_path();
        let pinned_group_names = load_config(&config_path)
            .map(|config| config.pinned_group_names.into_iter().collect())
            .unwrap_or_default();

        Self {
            proxy_groups: Vec::new(),
            selected_group_index: 0,
            selected_proxy_index: 0,
            rules: Vec::new(),
            selected_rule_index: 0,
            rule_filter: String::new(),
            editing_rule_filter: false,
            active_tab: ActiveTab::Proxy,
            focus: Focus::GroupList,
            api_url,
            secret,
            delay_refreshing: false,
            pinned_group_names,
            pending_g: false,
            config_path,
            show_help: false,
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
                    let delay = response
                        .proxies
                        .get(proxy_name)
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

        self.sort_proxy_groups();

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

    fn delay_refresh_request(&self) -> Option<DelayRefreshRequest> {
        let Some(group) = self.proxy_groups.get(self.selected_group_index) else {
            return None;
        };

        let proxy_names: Vec<String> = group
            .proxies
            .iter()
            .map(|proxy| proxy.name.clone())
            .collect();

        if proxy_names.is_empty() {
            return None;
        }

        Some(DelayRefreshRequest {
            group_name: group.name.clone(),
            proxy_names,
            api_url: self.api_url.clone(),
            secret: self.secret.clone(),
        })
    }

    fn sort_proxy_groups(&mut self) {
        self.proxy_groups.sort_by(|a, b| {
            let a_pinned = self.pinned_group_names.contains(&a.name);
            let b_pinned = self.pinned_group_names.contains(&b.name);

            b_pinned.cmp(&a_pinned).then_with(|| a.name.cmp(&b.name))
        });
    }

    fn toggle_selected_group_pin(&mut self) {
        let Some(group) = self.proxy_groups.get(self.selected_group_index) else {
            return;
        };
        let group_name = group.name.clone();

        if !self.pinned_group_names.insert(group_name.clone()) {
            self.pinned_group_names.remove(&group_name);
        }

        self.sort_proxy_groups();
        if let Some(pos) = self
            .proxy_groups
            .iter()
            .position(|group| group.name == group_name)
        {
            self.selected_group_index = pos;
        }

        if let Err(err) = self.save_config() {
            eprintln!("Error saving config: {}", err);
        }
    }

    fn save_config(&self) -> Result<()> {
        save_config(
            &self.config_path,
            &AppConfig {
                pinned_group_names: self.pinned_group_names.iter().cloned().collect(),
            },
        )
    }

    fn jump_to_top(&mut self) {
        match self.focus {
            Focus::GroupList => {
                if !self.proxy_groups.is_empty() {
                    self.selected_group_index = 0;
                    self.sync_selected_proxy();
                }
            }
            Focus::ProxyList => {
                self.selected_proxy_index = 0;
            }
            Focus::RuleList => {
                let filtered = self.filtered_rule_indices();
                if let Some(first) = filtered.first() {
                    self.selected_rule_index = *first;
                }
            }
        }
    }

    fn jump_to_bottom(&mut self) {
        match self.focus {
            Focus::GroupList => {
                if !self.proxy_groups.is_empty() {
                    self.selected_group_index = self.proxy_groups.len() - 1;
                    self.sync_selected_proxy();
                }
            }
            Focus::ProxyList => {
                if let Some(group) = self.proxy_groups.get(self.selected_group_index) {
                    if !group.proxies.is_empty() {
                        self.selected_proxy_index = group.proxies.len() - 1;
                    }
                }
            }
            Focus::RuleList => {
                let filtered = self.filtered_rule_indices();
                if let Some(last) = filtered.last() {
                    self.selected_rule_index = *last;
                }
            }
        }
    }

    fn apply_delay_refresh_result(&mut self, result: DelayRefreshResult) {
        self.delay_refreshing = false;

        let Some(group) = self
            .proxy_groups
            .iter_mut()
            .find(|group| group.name == result.group_name)
        else {
            return;
        };

        for (proxy_name, delay) in result.delays {
            if let Some(proxy) = group
                .proxies
                .iter_mut()
                .find(|proxy| proxy.name == proxy_name)
            {
                proxy.delay = Some(delay);
            }
        }
    }

    async fn fetch_group_delays(
        api_url: String,
        secret: Option<String>,
        proxy_names: Vec<String>,
    ) -> Vec<(String, u32)> {
        let client = reqwest::Client::new();
        let mut tasks = JoinSet::new();

        for proxy_name in proxy_names {
            let client = client.clone();
            let api_url = api_url.clone();
            let secret = secret.clone();

            tasks.spawn(async move {
                Self::fetch_proxy_delay(&client, &api_url, secret.as_deref(), &proxy_name)
                    .await
                    .ok()
                    .map(|delay| (proxy_name, delay))
            });
        }

        let mut delays = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Ok(Some(delay)) = result {
                delays.push(delay);
            }
        }

        delays
    }

    async fn fetch_proxy_delay(
        client: &reqwest::Client,
        api_url: &str,
        secret: Option<&str>,
        proxy_name: &str,
    ) -> Result<u32> {
        let mut url = reqwest::Url::parse(&format!("{}/", api_url.trim_end_matches('/')))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("invalid Clash API URL"))?;
            segments.push("proxies").push(proxy_name).push("delay");
        }
        url.query_pairs_mut()
            .append_pair("timeout", "5000")
            .append_pair("url", "https://www.gstatic.com/generate_204");

        let mut request = client.get(url);
        if let Some(secret) = secret {
            request = request.header("Authorization", format!("Bearer {}", secret));
        }

        let response: DelayResponse = request.send().await?.json().await?;
        Ok(response.delay)
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

    fn toggle_proxy_focus(&mut self) {
        if self.editing_rule_filter {
            return;
        }
        self.focus = match self.focus {
            Focus::GroupList => Focus::ProxyList,
            Focus::ProxyList => Focus::GroupList,
            Focus::RuleList => Focus::RuleList,
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
                let haystack =
                    format!("{} {} {}", rule.rule_type, rule.payload, rule.proxy).to_lowercase();
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
        self.active_tab = ActiveTab::Rules;
        self.focus = Focus::RuleList;
        self.editing_rule_filter = true;
    }

    fn next_tab(&mut self) {
        if self.editing_rule_filter {
            return;
        }

        self.active_tab = match self.active_tab {
            ActiveTab::Proxy => ActiveTab::Rules,
            ActiveTab::Rules => ActiveTab::About,
            ActiveTab::About => ActiveTab::Proxy,
        };
        self.sync_focus_to_tab();
    }

    fn sync_focus_to_tab(&mut self) {
        match self.active_tab {
            ActiveTab::Proxy => {
                if self.focus == Focus::RuleList {
                    self.focus = Focus::GroupList;
                }
            }
            ActiveTab::Rules => {
                self.focus = Focus::RuleList;
            }
            ActiveTab::About => {}
        }
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
    let api_url =
        std::env::var("CLASH_API_URL").unwrap_or_else(|_| "http://127.0.0.1:9091".to_string());
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
    let (delay_tx, mut delay_rx) = mpsc::channel::<DelayRefreshResult>(4);

    loop {
        while let Ok(result) = delay_rx.try_recv() {
            app.apply_delay_refresh_result(result);
        }

        terminal.draw(|f| ui(f, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.show_help {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('?') => {
                                app.show_help = false;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.editing_rule_filter {
                        app.pending_g = false;
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

                    if !matches!(key.code, KeyCode::Char('g')) {
                        app.pending_g = false;
                    }

                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('?') => {
                            app.show_help = true;
                        }
                        KeyCode::Char('g') => {
                            if app.pending_g {
                                app.jump_to_top();
                                app.pending_g = false;
                            } else {
                                app.pending_g = true;
                            }
                        }
                        KeyCode::Char('G') => {
                            app.jump_to_bottom();
                        }
                        KeyCode::Char('r') => {
                            // 刷新数据
                            app.fetch_proxies().await?;
                            app.fetch_rules().await?;
                        }
                        KeyCode::Char('d') => {
                            if app.active_tab == ActiveTab::Proxy && !app.delay_refreshing {
                                if let Some(request) = app.delay_refresh_request() {
                                    app.delay_refreshing = true;

                                    let sender = delay_tx.clone();
                                    tokio::spawn(async move {
                                        let delays = App::fetch_group_delays(
                                            request.api_url,
                                            request.secret,
                                            request.proxy_names,
                                        )
                                        .await;

                                        let _ = sender
                                            .send(DelayRefreshResult {
                                                group_name: request.group_name,
                                                delays,
                                            })
                                            .await;
                                    });
                                }
                            }
                        }
                        KeyCode::Tab => {
                            app.next_tab();
                        }
                        KeyCode::Char('h') | KeyCode::Left => {
                            if app.active_tab == ActiveTab::Proxy {
                                app.toggle_proxy_focus();
                            }
                        }
                        KeyCode::Char('l') | KeyCode::Right => {
                            if app.active_tab == ActiveTab::Proxy {
                                app.toggle_proxy_focus();
                            }
                        }
                        KeyCode::Char('/') => {
                            app.start_rule_filter_edit();
                        }
                        KeyCode::Char('c') => {
                            if app.focus == Focus::RuleList {
                                app.clear_rule_filter();
                            }
                        }
                        KeyCode::Char('p') => {
                            if app.active_tab == ActiveTab::Proxy && app.focus == Focus::GroupList {
                                app.toggle_selected_group_pin();
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => match app.focus {
                            Focus::GroupList if app.active_tab == ActiveTab::Proxy => {
                                app.previous_group()
                            }
                            Focus::ProxyList if app.active_tab == ActiveTab::Proxy => {
                                app.previous_proxy()
                            }
                            Focus::RuleList if app.active_tab == ActiveTab::Rules => {
                                app.previous_rule()
                            }
                            _ => {}
                        },
                        KeyCode::Char('j') | KeyCode::Down => match app.focus {
                            Focus::GroupList if app.active_tab == ActiveTab::Proxy => {
                                app.next_group()
                            }
                            Focus::ProxyList if app.active_tab == ActiveTab::Proxy => {
                                app.next_proxy()
                            }
                            Focus::RuleList if app.active_tab == ActiveTab::Rules => {
                                app.next_rule()
                            }
                            _ => {}
                        },
                        KeyCode::Enter => {
                            match (app.active_tab, app.focus) {
                                (ActiveTab::Proxy, Focus::GroupList) => {
                                    app.focus = Focus::ProxyList;
                                }
                                (ActiveTab::Proxy, Focus::ProxyList) => {
                                    if let Some(group) =
                                        app.proxy_groups.get(app.selected_group_index)
                                    {
                                        if let Some(proxy) =
                                            group.proxies.get(app.selected_proxy_index)
                                        {
                                            let group_name = group.name.clone();
                                            let proxy_name = proxy.name.clone();

                                            if let Err(e) =
                                                app.select_proxy(&group_name, &proxy_name).await
                                            {
                                                eprintln!("Error selecting proxy: {}", e);
                                            } else {
                                                // 更新成功后刷新数据
                                                app.fetch_proxies().await?;
                                            }
                                        }
                                    }
                                }
                                _ => {}
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
    let area = f.area().inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);

    render_tabs(f, app, chunks[0]);

    match app.active_tab {
        ActiveTab::Proxy => render_proxy_tab(f, app, chunks[1]),
        ActiveTab::Rules => render_rule_list(f, app, chunks[1]),
        ActiveTab::About => render_about(f, app, chunks[1]),
    }

    render_footer(f, app, chunks[2]);

    if app.show_help {
        render_help_popup(f, app);
    }
}

fn render_tabs(f: &mut Frame, app: &App, area: Rect) {
    let tabs = Paragraph::new(vec![Line::from(vec![
        tab_span("Proxy", app.active_tab == ActiveTab::Proxy),
        Span::raw("  "),
        tab_span("Rules", app.active_tab == ActiveTab::Rules),
        Span::raw("  "),
        tab_span("About", app.active_tab == ActiveTab::About),
    ])])
    .block(panel_block("Clash TUI", false))
    .alignment(Alignment::Left);

    f.render_widget(tabs, area);
}

fn tab_span(label: &'static str, active: bool) -> Span<'static> {
    if active {
        Span::styled(
            format!(" {label} "),
            Style::default()
                .fg(Color::Black)
                .bg(Theme::accent())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(format!(" {label} "), Theme::muted())
    }
}

fn render_proxy_tab(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(area);

    render_group_list(f, app, chunks[0]);
    render_proxy_list(f, app, chunks[1]);
}

fn render_about(f: &mut Frame, app: &App, area: Rect) {
    let config_path = app.config_path.to_string_lossy();
    let secret_state = if app.secret.is_some() {
        "已配置"
    } else {
        "未配置"
    };
    let content = vec![
        Line::from(vec![
            Span::styled(
                "Clash TUI",
                Theme::accent_text().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), Theme::muted()),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Controller  ", Theme::muted()),
            Span::styled(app.api_url.as_str(), Theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Secret      ", Theme::muted()),
            Span::styled(secret_state, Theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Config      ", Theme::muted()),
            Span::styled(config_path.to_string(), Theme::text()),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Proxy       ", Theme::muted()),
            Span::raw("h/l 面板  j/k 行  Enter 进入/选择  p 置顶  d 后台测速"),
        ]),
        Line::from(vec![
            Span::styled("Rules       ", Theme::muted()),
            Span::raw("/ 过滤  c 清除过滤  Enter/Esc 完成输入"),
        ]),
        Line::from(vec![
            Span::styled("Global      ", Theme::muted()),
            Span::raw("Tab 切换页面  gg/G 首尾  r 刷新  q 退出"),
        ]),
    ];

    let about = Paragraph::new(content)
        .block(panel_block("About", false))
        .style(Theme::text());
    f.render_widget(about, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let tab = match app.active_tab {
        ActiveTab::Proxy => "Proxy",
        ActiveTab::Rules => "Rules",
        ActiveTab::About => "About",
    };
    let focus = match app.focus {
        Focus::GroupList => "Groups",
        Focus::ProxyList => "Proxies",
        Focus::RuleList => "Rules",
    };

    let mut spans = if app.editing_rule_filter {
        vec![
            Span::styled(
                " FILTER ",
                Style::default().fg(Color::Black).bg(Theme::warning()),
            ),
            Span::raw("  "),
            Span::styled("输入关键词", Theme::text()),
            Span::styled("  Enter/Esc 完成", Theme::muted()),
            Span::styled("  Backspace 删除", Theme::muted()),
        ]
    } else {
        vec![
            Span::styled(" tab ", Theme::muted()),
            Span::styled(tab, Theme::accent_text().add_modifier(Modifier::BOLD)),
            Span::styled("  focus ", Theme::muted()),
            Span::styled(focus, Theme::text()),
            Span::styled("  groups ", Theme::muted()),
            Span::styled(app.proxy_groups.len().to_string(), Theme::text()),
            Span::styled("  rules ", Theme::muted()),
            Span::styled(app.rules.len().to_string(), Theme::text()),
        ]
    };

    if app.delay_refreshing {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            " TESTING ",
            Style::default().fg(Color::Black).bg(Theme::warning()),
        ));
        spans.push(Span::styled(" 后台测速中", Theme::warning_text()));
    }

    let footer = Paragraph::new(Line::from(spans))
        .style(Theme::muted())
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Theme::border()),
        );
    f.render_widget(footer, area);
}

fn render_help_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(64, 72, f.area());
    f.render_widget(Clear, area);

    let current_tab = match app.active_tab {
        ActiveTab::Proxy => "Proxy",
        ActiveTab::Rules => "Rules",
        ActiveTab::About => "About",
    };

    let content = vec![
        Line::from(vec![
            Span::styled("Current page  ", Theme::muted()),
            Span::styled(
                current_tab,
                Theme::accent_text().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        cheat_line("Tab", "切换 Proxy / Rules / About 页面"),
        cheat_line("?", "打开或关闭这个快捷键列表"),
        cheat_line("Esc", "关闭弹窗；过滤输入中结束输入"),
        cheat_line("q", "退出程序"),
        cheat_line("r", "刷新代理与规则数据"),
        cheat_line("gg / G", "跳到当前列表顶部 / 底部"),
        Line::raw(""),
        Line::from(Span::styled(
            "Proxy",
            Theme::accent_text().add_modifier(Modifier::BOLD),
        )),
        cheat_line("h / l, ← / →", "在代理组和代理列表之间切换"),
        cheat_line("j / k, ↑ / ↓", "移动当前列表光标"),
        cheat_line("Enter", "代理组进入代理列表；代理列表选择节点"),
        cheat_line("p", "置顶或取消置顶当前代理组"),
        cheat_line("d", "后台刷新当前代理组延迟"),
        Line::raw(""),
        Line::from(Span::styled(
            "Rules",
            Theme::accent_text().add_modifier(Modifier::BOLD),
        )),
        cheat_line("/", "进入 Rules 页并开始过滤"),
        cheat_line("c / Delete", "清空规则过滤"),
        cheat_line("Backspace", "过滤输入中删除字符"),
    ];

    let popup = Paragraph::new(content)
        .block(panel_block("Cheatlist", true))
        .style(Theme::text());
    f.render_widget(popup, area);
}

fn cheat_line(key: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<14}"), Theme::warning_text()),
        Span::styled(description, Theme::text()),
    ])
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);

    horizontal[1]
}

fn panel_block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            Theme::focused_border()
        } else {
            Theme::border()
        })
        .title(title.into())
        .title_style(if focused {
            Theme::accent_text().add_modifier(Modifier::BOLD)
        } else {
            Theme::muted()
        })
        .padding(Padding::horizontal(1))
}

struct Theme;

impl Theme {
    fn text() -> Style {
        Style::default().fg(Color::Rgb(225, 229, 235))
    }

    fn muted() -> Style {
        Style::default().fg(Color::Rgb(116, 126, 140))
    }

    fn border() -> Style {
        Style::default().fg(Color::Rgb(55, 65, 81))
    }

    fn focused_border() -> Style {
        Style::default().fg(Self::accent())
    }

    fn accent() -> Color {
        Color::Rgb(245, 190, 96)
    }

    fn accent_text() -> Style {
        Style::default().fg(Self::accent())
    }

    fn selected() -> Style {
        Style::default()
            .fg(Color::Rgb(245, 248, 255))
            .bg(Color::Rgb(38, 52, 67))
            .add_modifier(Modifier::BOLD)
    }

    fn success() -> Style {
        Style::default().fg(Color::Rgb(91, 214, 139))
    }

    fn warning() -> Color {
        Color::Rgb(255, 214, 128)
    }

    fn warning_text() -> Style {
        Style::default().fg(Self::warning())
    }

    fn danger() -> Style {
        Style::default().fg(Color::Rgb(255, 120, 120))
    }
}

fn delay_style(delay: Option<u32>) -> Style {
    match delay {
        Some(ms) if ms <= 120 => Theme::success(),
        Some(ms) if ms <= 350 => Theme::warning_text(),
        Some(_) => Theme::danger(),
        None => Theme::muted(),
    }
}

fn delay_label(delay: Option<u32>) -> String {
    match delay {
        Some(ms) => format!("{ms:>4} ms"),
        None => "   - ms".to_string(),
    }
}

fn focus_label(focused: bool) -> Span<'static> {
    if focused {
        Span::styled(
            " ACTIVE ",
            Style::default().fg(Color::Black).bg(Theme::accent()),
        )
    } else {
        Span::styled("       ", Theme::muted())
    }
}

fn render_empty(
    f: &mut Frame,
    area: Rect,
    title: &'static str,
    focused: bool,
    message: &'static str,
) {
    let empty = Paragraph::new(Line::from(vec![
        Span::styled("  ", Theme::muted()),
        Span::styled(message, Theme::muted()),
    ]))
    .block(panel_block(title, focused))
    .alignment(Alignment::Center);
    f.render_widget(empty, area);
}

fn render_group_list(f: &mut Frame, app: &App, area: Rect) {
    if app.proxy_groups.is_empty() {
        render_empty(
            f,
            area,
            "代理组",
            app.focus == Focus::GroupList,
            "暂无代理组，按 r 刷新",
        );
        return;
    }

    let items: Vec<ListItem> = app
        .proxy_groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let current = group.current.as_deref().unwrap_or("未选择");
            let is_selected = i == app.selected_group_index;
            let is_pinned = app.pinned_group_names.contains(&group.name);
            let mut line = Line::from(vec![
                Span::styled(if is_selected { "● " } else { "  " }, Theme::accent_text()),
                Span::styled(if is_pinned { "★ " } else { "  " }, Theme::warning_text()),
                Span::styled(group.name.clone(), Theme::text()),
                Span::styled("  ", Theme::muted()),
                Span::styled(current.to_string(), Theme::muted()),
            ]);

            if is_selected {
                line = line.style(if app.focus == Focus::GroupList {
                    Theme::selected()
                } else {
                    Theme::accent_text().add_modifier(Modifier::BOLD)
                });
            }

            ListItem::new(line)
        })
        .collect();

    let title = Line::from(vec![
        Span::raw(" 代理组 "),
        Span::styled(
            format!(
                "置顶 {} ",
                app.proxy_groups
                    .iter()
                    .filter(|group| app.pinned_group_names.contains(&group.name))
                    .count()
            ),
            Theme::muted(),
        ),
        focus_label(app.focus == Focus::GroupList),
    ]);

    let list = List::new(items)
        .block(panel_block(title, app.focus == Focus::GroupList))
        .highlight_symbol("▌ ")
        .highlight_style(if app.focus == Focus::GroupList {
            Theme::selected()
        } else {
            Theme::accent_text()
        });

    let mut state = ListState::default();
    state.select(Some(app.selected_group_index));

    f.render_stateful_widget(list, area, &mut state);
}

fn render_proxy_list(f: &mut Frame, app: &App, area: Rect) {
    let Some(group) = app.proxy_groups.get(app.selected_group_index) else {
        render_empty(
            f,
            area,
            "代理",
            app.focus == Focus::ProxyList,
            "先选择代理组",
        );
        return;
    };

    if group.proxies.is_empty() {
        render_empty(
            f,
            area,
            "代理",
            app.focus == Focus::ProxyList,
            "当前组没有可选代理",
        );
        return;
    }

    let items: Vec<ListItem> = group
        .proxies
        .iter()
        .enumerate()
        .map(|(i, proxy)| {
            let is_current = group.current.as_ref() == Some(&proxy.name);
            let is_selected = i == app.selected_proxy_index;
            let marker = if is_current { "✓" } else { " " };
            let mut line = Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    if is_current {
                        Theme::success()
                    } else {
                        Theme::muted()
                    },
                ),
                Span::styled(delay_label(proxy.delay), delay_style(proxy.delay)),
                Span::styled("  ", Theme::muted()),
                Span::styled(
                    proxy.name.clone(),
                    if is_current {
                        Theme::success()
                    } else {
                        Theme::text()
                    },
                ),
            ]);

            if is_selected {
                line = line.style(if app.focus == Focus::ProxyList {
                    Theme::selected()
                } else {
                    Theme::accent_text().add_modifier(Modifier::BOLD)
                });
            }

            ListItem::new(line)
        })
        .collect();

    let title = Line::from(vec![
        Span::raw(" 代理 "),
        Span::styled(group.name.clone(), Theme::muted()),
        Span::raw(" "),
        focus_label(app.focus == Focus::ProxyList),
    ]);

    let list = List::new(items)
        .block(panel_block(title, app.focus == Focus::ProxyList))
        .highlight_symbol("▌ ")
        .highlight_style(if app.focus == Focus::ProxyList {
            Theme::selected()
        } else {
            Theme::accent_text()
        });

    let mut state = ListState::default();
    state.select(Some(app.selected_proxy_index));

    f.render_stateful_widget(list, area, &mut state);
}

fn render_rule_list(f: &mut Frame, app: &App, area: Rect) {
    let filtered_indices = app.filtered_rule_indices();

    if app.rules.is_empty() {
        render_empty(
            f,
            area,
            "规则",
            app.focus == Focus::RuleList,
            "暂无规则，按 r 刷新",
        );
        return;
    }

    if filtered_indices.is_empty() {
        render_empty(
            f,
            area,
            "规则",
            app.focus == Focus::RuleList,
            "没有匹配的规则",
        );
        return;
    }

    let items: Vec<ListItem> = filtered_indices
        .iter()
        .map(|&idx| {
            let rule = &app.rules[idx];
            let is_selected = app.selected_rule_index == idx;
            let mut line = Line::from(vec![
                Span::styled(format!("{:<11}", rule.rule_type), Theme::accent_text()),
                Span::styled("  ", Theme::muted()),
                Span::styled(rule.payload.clone(), Theme::text()),
                Span::styled("  ->  ", Theme::muted()),
                Span::styled(rule.proxy.clone(), Theme::success()),
            ]);

            if is_selected {
                line = line.style(if app.focus == Focus::RuleList {
                    if app.editing_rule_filter {
                        Theme::selected().add_modifier(Modifier::UNDERLINED)
                    } else {
                        Theme::selected()
                    }
                } else {
                    Theme::accent_text().add_modifier(Modifier::BOLD)
                });
            }

            ListItem::new(line)
        })
        .collect();

    let filter = if app.rule_filter.is_empty() {
        Span::styled(" / 过滤", Theme::muted())
    } else {
        Span::styled(format!(" / {}", app.rule_filter), Theme::warning_text())
    };
    let edit = if app.editing_rule_filter {
        Span::styled(
            " EDIT ",
            Style::default().fg(Color::Black).bg(Theme::warning()),
        )
    } else {
        Span::raw("")
    };

    let title = Line::from(vec![
        Span::raw(" 规则 "),
        Span::styled(
            format!("{} / {}", filtered_indices.len(), app.rules.len()),
            Theme::muted(),
        ),
        filter,
        Span::raw(" "),
        edit,
        focus_label(app.focus == Focus::RuleList),
    ]);

    let list = List::new(items)
        .block(panel_block(title, app.focus == Focus::RuleList))
        .highlight_symbol("▌ ")
        .highlight_style(if app.focus == Focus::RuleList {
            Theme::selected()
        } else {
            Theme::accent_text()
        });

    let mut state = ListState::default();
    let selected_position = filtered_indices
        .iter()
        .position(|&idx| idx == app.selected_rule_index);
    state.select(selected_position);

    f.render_stateful_widget(list, area, &mut state);
}
