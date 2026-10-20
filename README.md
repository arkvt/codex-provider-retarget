# Codex Provider Retarget（TUI）

一个用于修复「Codex 切换 Provider 后历史会话不可见」的本地终端工具。
用静态 CRT 构建后是单个可执行文件，界面为终端全屏 TUI，可直接分发，不需要安装器。

## 项目结构

```
.
├── Cargo.toml
└── src/
    ├── main.rs            终端生命周期、事件循环、按键映射
    ├── app.rs             状态机（不依赖终端，可单元测试）
    ├── ui.rs              渲染（可用 TestBackend 做快照测试）
    └── core/
        ├── types.rs       领域类型、迁移阶段、源 Provider 解析
        ├── paths.rs       Codex 目录定位与规范化
        ├── config.rs      config.toml 读取 model_provider
        ├── database.rs    state_5.sqlite 分布统计与事务更新
        ├── sessions.rs    sessions/**/*.jsonl 扫描与重写
        ├── backup.rs      时间戳备份目录与递归复制
        ├── processes.rs   Codex 进程检测（sysinfo，跨平台）
        ├── scan.rs        扫描聚合
        └── migrate.rs     迁移编排与进度回调
```

## 为什么需要它

Codex 只显示「历史记录里保存的 Provider」与「当前 `config.toml` 里的 Provider」一致的历史会话。
切换 Provider 之后，文件还在磁盘上，但 Codex 界面不再展示它们：

```text
当前 Codex Provider == 历史记录中的 Provider   →  会话可见
```

典型场景：原先用官方 OpenAI（历史里是 `openai`），后来切到自定义 Provider（例如 `ccswitch`），
旧会话就在界面上消失了。本工具改写的正是**历史记录里保存的那个 Provider 名称**。

## 它改什么

| 位置 | 字段 | 说明 |
| --- | --- | --- |
| `<codex-home>/state_5.sqlite` | `threads.model_provider` | 单事务 UPDATE，随后 `wal_checkpoint(TRUNCATE)` |
| `<codex-home>/sessions/**/*.jsonl` | `session_meta` 记录的 `payload.model_provider` | 只替换目标行，其余行逐字保留 |

写入前会在 `<codex-home>/backups/provider-migration-YYYYMMDD-HHMMSS/` 下创建完整备份
（`state_5.sqlite`、`-wal`、`-shm` 以及整个 `sessions/`）。

## 构建与运行

```bash
cargo build --release
./target/release/codex-retarget        # Windows: .\target\release\codex-retarget.exe
```

也可以直接 `cargo run --release`。程序**不接受任何命令行参数**：传入参数会提示并退出。
需要一个真正的交互式终端（非交互环境下会给出明确报错，而不是 panic）。

### 单文件分发（推荐）

默认的 MSVC 构建会动态链接 VC++ 运行库（`VCRUNTIME140.dll`）。要得到一个不依赖任何运行库、
可以直接拷到别的 64 位 Windows 机器上运行的单文件，用静态 CRT 构建：

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release
```

发布用的 `dist/codex-retarget.exe` 就是这样构建后复制出来的：

| 项目 | 值 |
| --- | --- |
| 大小 | 1.97 MB |
| 目标 | `x86_64-pc-windows-msvc`（64 位 Windows） |
| 外部 DLL 依赖 | 仅 `KERNEL32` / `USER32` / `bcrypt` / `ntdll`（均为系统自带） |
| 内置 | SQLite 已静态编入，无需系统 sqlite3、无需 VC++ 运行库 |
| SHA256 | `DDE05630A1F3C8D2A8EA22CD1612B3DF38B42449C3051E88E0C07C6CE8AB7C0B` |

注意：不带 `RUSTFLAGS` 重新 `cargo build --release` 会把 `target/release/codex-retarget.exe`
覆盖回动态 CRT 版本；`dist/` 里的副本不受影响。

依赖只有 7 个 crate：`ratatui`、`rusqlite`（`bundled`，自带 SQLite 源码，无需系统库）、
`serde`、`serde_json`、`toml`、`chrono`、`sysinfo`。

## 界面

界面为全中文。下面是在 96x20（最小支持尺寸）下由测试真实渲染出来的画面：

```
┌ Codex 历史 Provider 迁移工具 v0.1.0 ─────────────────────────────────────────────────────────┐
│目录  C:\Users\me\.codex   [p] 修改目录   [r] 重新扫描                                        │
│目标  ccswitch   状态  就绪                                                                   │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
┌ 提供方 - 已选 2/2 ───────────────────────────────┐┌ 概览 ────────────────────────────────────┐
│         提供方            数据库  Session  合计  ││线程记录  1205 条（state_5.sqlite）       │
│> 目标   ccswitch          5       0        5     ││会话文件  436 个（sessions/）             │
│  [x]    codex             0       36       36    ││当前配置  ccswitch                        │
│  [x]    openai            1200    400      1600  ││                                          │
│                                                  ││待更新  1200 条线程记录、436 个会话文件   │
│                                                  ││                                          │
└──────────────────────────────────────────────────┘└──────────────────────────────────────────┘
┌ 迁移审计 ────────────────────────────────────────────────────────────────────────────────────┐
│  .  扫描配置                                                                                 │
│  .  完整备份                                                                                 │
│  .  更新数据库                                                                               │
│  .  更新 Session Metadata                                                                    │
│  .  校验结果                                                                                 │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
 空格 勾选   a 全选   n 清空   t 目标   p 目录   m 迁移   r 重扫   ? 帮助   q 退出
```

### 快捷键

| 键 | 作用 |
| --- | --- |
| `空格` | 勾选 / 取消当前提供方 |
| `a` / `n` | 全选 / 全不选（目标 Provider 不可选） |
| `t` / `p` | 修改目标 Provider / Codex 目录 |
| `m` | 打开确认弹窗，按 `y` 开始迁移 |
| `r` | 重新扫描 Codex 目录 |
| `j` `k` / `↑` `↓` / `PageUp` `PageDown` / `Home` `End` | 移动光标 |
| `?` `h` | 帮助 |
| `q` / `esc` / `Ctrl+C` | 退出（有弹窗时先关闭弹窗） |

### 终端要求

- 界面用 Unicode 框线和汉字，**建议使用 Windows Terminal**；Windows 自带的 conhost 需要把字体设成
  等宽中文字体（如「更纱黑体」「微软雅黑」「新宋体」），否则可能显示成方块。
- 程序通过 Rust 标准输出直接调用 `WriteConsoleW` 写出 Unicode，不依赖控制台代码页，
  因此 CP936 环境下也不会乱码。
- 最小可用窗口为 **96x20**；更小的窗口会显示提示而不是画出错乱的界面。
- 受终端限制，**输入框无法用输入法打中文**（TUI 收不到 IME 组合输入）。但 Codex 目录
  默认会自动探测，也可以用 `~` 简写，正常使用不需要手打中文路径。

### 预览界面

测试里内置了快照预览，不用真的启动程序就能把每个界面打印出来：

```powershell
$env:SHOW_TUI = "1"
cargo test every_overlay_renders_at_the_minimum_size -- --nocapture
```


## 迁移流程

点 `m` → `y` 之后按顺序执行，审计区实时反映进度：

1. **扫描配置** — 读取 Codex 目录、`config.toml`、`state_5.sqlite`、`sessions/`
2. **完整备份** — 时间戳目录，包含数据库、wal/shm、整个 `sessions/`
3. **更新数据库** — 单事务更新 `threads.model_provider`，再 checkpoint WAL
4. **更新 Session Metadata** — 逐个重写 `session_meta` 记录
5. **验证** — 迁移完成后自动重新扫描，结果页展示 Provider 分布的前后对比

## 安全设计

- **进程保护**：检测到 Codex CLI / Codex App 在运行就拒绝写入（数据库与 session 文件可能正被占用）。
  关闭后按 `r` 重新检测。本工具**不会**替你结束进程。
- **文件占用保护**：扫描阶段如果发现 session 文件读取失败（被其他进程占用、或内容损坏），
  概览区会立刻给出提示；此时**迁移会被直接拒绝**。原因不是「怕失败」，而是
  「先读后写」之间如果写入方又追加了内容，重写会截断这些新数据。
  提示会说明是哪个目录下的多少个文件有问题，关闭占用方后重新扫描即可。
- **先备份后写入**：备份失败即中止，不会出现「改了一半没有备份」。
- **失败不中断**：迁移过程中单个 session 文件读取/写入失败会被收集，继续处理其余文件，
  最后在结果页列出。
- **不做隐式扩大**：迁移范围严格等于你勾选的名称。空选择只会报错，绝不会退化成「迁移全部」。
- **本地执行**：不上传任何数据，无遥测，不读取账号凭据。

### 关于 Codex 的数据库触发器

当前版本的 `state_5.sqlite` 里 `threads` 表带有若干触发器，但都是**列级**触发
（`AFTER UPDATE OF created_at` / `AFTER UPDATE OF updated_at`，且带
`WHEN NEW.x != OLD.x` 条件）。本工具只更新 `model_provider`，因此这些触发器不会触发，
`updated_at` / `recency_at` 不会被改动，历史记录的排序不会因为迁移而变化。

### 已知范围限制

- 只处理 `<codex-home>/sessions/`，不处理 `archived_sessions/`。
- 只处理 `config.toml` **顶层**的 `model_provider`；`[model_providers.*]` 这类表不受影响，
  也无需改动。
- 数据库层面只改 `threads.model_provider`，不改动 `rollout_path` 等其他列。

## 设计取舍

- **单一 TUI，单文件分发**：界面即终端，构建产物只有一个可执行文件；SQLite 直接静态编入，不依赖原生模块或安装器。
- **不提供一键结束进程**：检测到 Codex CLI / Codex App 正在运行只阻止写入，由用户自行关闭，避免中断正在运行的 Codex 任务。
- **不删除 WAL/SHM**：数据库更新只做事务提交和 `wal_checkpoint(TRUNCATE)`，不删除 `-wal` / `-shm`；即使 checkpoint 因 busy 失败，已提交但仍在 WAL 中的数据也不会丢失。
- **JSONL 只替换目标行**：逐行处理并保留原始行尾（CRLF/LF），目标行之外的字节逐字复制，不会把整个文件的行尾规范化。
- **空选择即报错**：迁移范围严格等于用户勾选的名称，空选择直接拒绝，绝不退化成「迁移全部」。
- **单文件失败不中断**：某个 session 文件读写失败会被收集，其余文件继续处理，最后在结果页列出。
- **文件占用检查**：扫描阶段统计无法读取的 session 文件；迁移前若存在，直接拒绝，避免与仍在写入该文件的 Codex 宿主竞争。
- **进程检测直接查询操作系统**：不解析外部命令输出，避免受输出格式、本地化和平台差异影响；只刷新 `cmd` 与 `exe`，不解析用户账户信息——`ProcessRefreshKind::everything()` 在域/网络环境下会为每个进程解析账户，实测能让一次检测阻塞 20 分钟以上。

## 测试

```bash
cargo test
```

覆盖范围：

- `core::config` — 正常/缺失/空值/损坏 TOML
- `core::sessions` — 递归扫描、`session_meta` 识别、保留 CRLF、无结尾换行、只改目标行、
  不可读文件单独上报
- `core::database` — 分布统计、忽略 NULL、事务更新、大小写敏感匹配、缺 `threads` 表
- `core::backup` — 时间戳唯一性、wal/shm 复制、嵌套目录镜像
- `core::scan` — 完整目录、空目录、损坏数据库仍能扫描 sessions、不可读 session 单独上报
- `core::migrate` — 端到端迁移（含备份校验）、只迁移勾选项、进程阻止时零写入、
  不可读文件阻止时零写入、空选择拒绝、缺少数据库拒绝
- `core::processes` — 进程名/命令行识别、父子进程折叠、环状父链终止
- `app` — 选择默认值、目标变更重算、迁移门禁、按键路由、弹窗状态、日志上限
- `ui` — 用 `TestBackend` 渲染中文快照，断言各面板与全部弹窗内容，并专门守三类
  「纯宽度检查抓不到」的排版问题：
  - `wide_characters_keep_the_frame_aligned` — 每行必须正好是指定列数（汉字占两列，边框不能歪）
  - `panel_borders_are_never_overwritten` — 文字比栏位宽时会把右边框盖掉，这里逐行检查边界完整
  - `overlays_are_not_clipped_at_the_minimum_size` — 帮助与结果弹窗在最小窗口下必须完整可读

## 本地化说明

界面文案全部为中文，集中在两处，方便后续增删语言：

- `src/ui.rs` — 面板标题、表头、弹窗、底部提示
- `src/core/*.rs` 与 `src/app.rs` — 进度标签、状态文本、错误信息

代码标识符与注释保持英文。渲染层不依赖任何 i18n 库：所有文案都是直接写死的字符串字面量，
汉字宽度由 ratatui 依据 `unicode-width` 计算，双宽字符后的那一格会被自动跳过，
因此边框不会因为中英文混排而错位。

## License

MIT
