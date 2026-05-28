# Clash TUI

一个使用 Rust + Ratatui 构建的终端界面，可与 Clash 控制面板交互，便捷地查看、切换代理，并浏览/筛选当前 Clash 规则。

## 功能特性

- 查看所有代理组及其当前选中的代理
- 浏览代理组下所有可选代理并显示最近延迟
- 选择代理并立即调用 Clash API 切换
- 显示 Clash 返回的规则列表，支持实时关键词过滤
- 在三个面板之间快速切换焦点，支持刷新数据

## 快速开始

```bash
cargo run
```

构建发布版本：
```bash
cargo build --release
```

## 配置

程序通过环境变量读取 Clash API 信息：

| 环境变量 | 说明 | 默认值 |
| --- | --- | --- |
| `CLASH_API_URL` | Clash External Controller 地址 | `http://127.0.0.1:9091` |
| `CLASH_SECRET`  | Clash API 密钥（若 `secret` 已启用则必填） | 空 |

示例：
```bash
export CLASH_API_URL="http://127.0.0.1:9091"
export CLASH_SECRET="your-secret"
cargo run
```

## 键盘操作

| 按键 | 作用 |
| --- | --- |
| `Tab` | 在 `Proxy` / `Rules` / `About` 顶部页面之间切换 |
| `?` | 弹出或关闭快捷键 cheatlist |
| `h` / `l` 或 `←` / `→` | 在 `Proxy` 页的代理组和代理面板之间切换 |
| `↑` / `↓` 或 `k` / `j` | 在当前焦点面板中移动光标 |
| `gg` / `G` | 跳到当前焦点列表顶部 / 底部 |
| `Enter` | 在代理组面板进入代理面板；在代理面板选中当前代理并调用 Clash API 切换 |
| `p` | 在代理组面板切换当前代理组置顶，可同时置顶多个 |
| `d` | 在后台对当前代理组内的节点刷新延迟测试 |
| `r` | 同步刷新代理与规则数据 |
| `/` | 跳到 `Rules` 页并开始输入过滤关键词 |
| `Backspace` | 在规则过滤输入模式下删除最后一个字符 |
| `Delete` 或 `c` | 清空规则过滤（`c` 仅在规则面板有效） |
| `Enter` / `Esc` | 结束规则过滤输入模式 |
| `q` | 退出程序 |

## 界面概览

```
┌───────────────────────────────────────────────┐
│ Clash TUI     Proxy  Rules  About            │
└───────────────────────────────────────────────┘
┌──────────────────────┬────────────────────────┐
│ 代理组               │ 代理 - GLOBAL          │
│ ...                  │ ✓ DIRECT (N/A)         │
│                      │   x1 香港 - 10 ms      │
│                      │   ...                  │
└──────────────────────┴────────────────────────┘
┌───────────────────────────────────────────────┐
│ tab Proxy  focus Groups  groups 12  rules 560 │
└───────────────────────────────────────────────┘
```

- 黄色边框表示当前焦点面板；同一面板中黄色高亮的行即当前光标位置。
- 顶部为 `Proxy` / `Rules` / `About` 页面；`Proxy` 页只显示代理组和代理列表。
- 底部只显示当前状态；完整快捷键列表通过 `?` 弹窗查看。
- 代理组可通过 `p` 多选置顶，置顶组会排在普通组前面，并持久化到 `~/.config/clash_tui/config.json`。
- 过滤后的规则标题会显示“过滤: 关键词”以及当前匹配条目数。
- 规则过滤支持匹配规则类型、payload 与目标代理，大小写不敏感。

## 依赖

- [ratatui](https://github.com/ratatui-org/ratatui)：终端 UI 框架
- [crossterm](https://github.com/crossterm-rs/crossterm)：跨平台终端 I/O
- [tokio](https://tokio.rs)：异步运行时
- [reqwest](https://github.com/seanmonstar/reqwest)：HTTP 客户端
- [serde](https://serde.rs)：序列化/反序列化
- [anyhow](https://github.com/dtolnay/anyhow)：错误处理

## 调用的 Clash API

- `GET /proxies`：获取代理组及节点状态
- `PUT /proxies/{group}`：切换代理组内当前节点
- `GET /rules`：查询当前规则链

确保 Clash 已启用 `external-controller`，并在 `allow-lan` 或本机可访问的前提下运行。若配置了 `secret`，请同步设置 `CLASH_SECRET`。

## 许可证

MIT License
