//! 渲染层。这里全部是 [`App`] 的纯函数，因此整个界面都能用 `TestBackend` 做快照测试。

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, TableState, Wrap};

use crate::app::{App, Busy, EditField, Overlay, ProviderRow, StageState};
use crate::core::types::{ProviderDistribution, Stage};

const ACCENT: Color = Color::Rgb(124, 140, 255);
const SUCCESS: Color = Color::Rgb(68, 199, 182);
const WARNING: Color = Color::Rgb(226, 169, 94);
const MUTED: Color = Color::DarkGray;

/// 中文占两列，双栏布局又要把边框算进去，所以最小宽度留足余量。
const MIN_WIDTH: u16 = 96;
const MIN_HEIGHT: u16 = 20;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "终端窗口太小",
                    Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
                )),
                Line::from(format!(
                    "至少需要 {MIN_WIDTH}x{MIN_HEIGHT}，当前 {}x{}。",
                    area.width, area.height
                )),
                Line::from(Span::styled(
                    "请把窗口拉大，或调小终端字号。",
                    Style::default().fg(MUTED),
                )),
            ])
            .block(Block::bordered().title(" Codex 历史 Provider 迁移工具 ")),
            area,
        );
        return;
    }

    let [header, body, audit, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(8),
        Constraint::Length(7),
        Constraint::Length(1),
    ])
    .areas(area);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(54), Constraint::Percentage(46)]).areas(body);

    draw_header(frame, header, app);
    draw_providers(frame, left, app);
    draw_overview(frame, right, app);
    draw_audit(frame, audit, app);
    draw_footer(frame, footer, app);

    match app.overlay {
        Some(Overlay::Help) => draw_help(frame, area),
        Some(Overlay::Confirm) => draw_confirm(frame, area, app),
        Some(Overlay::Blocked) => draw_blocked(frame, area, app),
        Some(Overlay::Report) => draw_report(frame, area, app),
        Some(Overlay::Error) => draw_error(frame, area, app),
        Some(Overlay::Edit(field)) => draw_edit(frame, area, app, field),
        None => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let target = if app.target_provider.trim().is_empty() {
        Span::styled("<未设置 - 按 t 输入>", Style::default().fg(WARNING))
    } else {
        Span::styled(
            app.target_provider.clone(),
            Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
        )
    };

    let status_color = match app.busy {
        Busy::Idle if app.error.is_some() => Color::Red,
        Busy::Idle if app.outcome.is_some() => SUCCESS,
        Busy::Idle => MUTED,
        _ => ACCENT,
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("目录  ", Style::default().fg(MUTED)),
            Span::styled(app.codex_home.clone(), Style::default().fg(Color::White)),
            Span::styled("   [p] 修改目录   [r] 重新扫描", Style::default().fg(MUTED)),
        ]),
        Line::from(vec![
            Span::styled("目标  ", Style::default().fg(MUTED)),
            target,
            Span::styled("   状态  ", Style::default().fg(MUTED)),
            Span::styled(app.status_text(), Style::default().fg(status_color)),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(format!(
                    " Codex 历史 Provider 迁移工具 v{} ",
                    env!("CARGO_PKG_VERSION")
                ))
                .title_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
                .border_style(Style::default().fg(ACCENT)),
        ),
        area,
    );
}

fn draw_providers(frame: &mut Frame, area: Rect, app: &App) {
    let widths = [
        Constraint::Length(6),
        Constraint::Min(12),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(6),
    ];

    let header = Row::new(vec![
        Line::from(""),
        Line::from(Span::styled("提供方", Style::default().fg(MUTED))),
        Line::from(Span::styled("数据库", Style::default().fg(MUTED))),
        Line::from(Span::styled("Session", Style::default().fg(MUTED))),
        Line::from(Span::styled("合计", Style::default().fg(MUTED))),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = app
        .rows
        .iter()
        .map(|row| provider_row(row, app))
        .collect();

    let title = if app.rows.is_empty() {
        " 提供方 - 没有发现任何记录 ".to_string()
    } else {
        format!(
            " 提供方 - 已选 {}/{} ",
            app.selected_sources().len(),
            app.selectable_count()
        )
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::bordered()
                .title(title)
                .border_style(Style::default().fg(ACCENT)),
        )
        .column_spacing(1)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut state = TableState::new();
    if !app.rows.is_empty() {
        state.select(Some(app.cursor.min(app.rows.len() - 1)));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn provider_row(row: &ProviderRow, app: &App) -> Row<'static> {
    let is_target = app.is_target(row);

    let marker = if is_target {
        Span::styled(
            "目标",
            Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
        )
    } else if row.selected {
        Span::styled("[x]", Style::default().fg(SUCCESS))
    } else {
        Span::styled("[ ]", Style::default().fg(MUTED))
    };

    let name = Line::from(Span::styled(
        row.name.clone(),
        if is_target {
            Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        },
    ));

    let count_style = if is_target {
        Style::default().fg(MUTED)
    } else if row.selected {
        Style::default().fg(SUCCESS)
    } else {
        Style::default().fg(MUTED)
    };

    Row::new(vec![
        Line::from(marker),
        name,
        Line::from(Span::styled(row.database_count.to_string(), count_style)),
        Line::from(Span::styled(row.session_count.to_string(), count_style)),
        Line::from(Span::styled(
            row.total().to_string(),
            count_style.add_modifier(Modifier::BOLD),
        )),
    ])
}

fn draw_overview(frame: &mut Frame, area: Rect, app: &App) {
    let (database_rows, session_files) = app.pending_counts();
    let mut lines: Vec<Line> = Vec::new();

    match &app.scan {
        None => lines.push(Line::from(Span::styled(
            "正在扫描 Codex 目录…",
            Style::default().fg(MUTED),
        ))),
        Some(scan) => {
            lines.push(stat_line(
                "线程记录",
                if scan.database_exists {
                    format!("{} 条（state_5.sqlite）", scan.thread_count)
                } else {
                    "未找到 state_5.sqlite".to_string()
                },
                scan.database_exists,
            ));
            lines.push(stat_line(
                "会话文件",
                if scan.sessions_exists {
                    format!("{} 个（sessions/）", scan.session_count)
                } else {
                    "未找到 sessions 目录".to_string()
                },
                scan.sessions_exists,
            ));
            lines.push(stat_line(
                "当前配置",
                scan.config_provider
                    .clone()
                    .unwrap_or_else(|| "config.toml 里没有 model_provider".to_string()),
                scan.config_provider.is_some(),
            ));

            if !scan.unreadable_sessions.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "! {} 个 session 文件无法读取 - 请关闭占用它们的程序后重新扫描",
                        scan.unreadable_sessions.len()
                    ),
                    Style::default().fg(WARNING),
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("待更新  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("{database_rows} 条线程记录、{session_files} 个会话文件"),
            Style::default().fg(if database_rows + session_files > 0 {
                SUCCESS
            } else {
                MUTED
            }),
        ),
    ]));

    if let Some(outcome) = &app.outcome {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("上次结果  ", Style::default().fg(MUTED)),
            Span::styled(
                format!(
                    "已更新 {} 条记录、{} 个文件",
                    outcome.updated_thread_rows, outcome.updated_session_files
                ),
                Style::default().fg(SUCCESS),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("备份目录  ", Style::default().fg(MUTED)),
            Span::styled(
                outcome.backup_dir.to_string_lossy().to_string(),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    if let Some(scan) = &app.scan
        && !scan.errors.is_empty()
    {
        lines.push(Line::from(""));
        for error in scan.errors.iter().take(3) {
            lines.push(Line::from(Span::styled(
                format!("! {error}"),
                Style::default().fg(WARNING),
            )));
        }
    }

    if let Some(blocker) = app.migration_blocker() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("无法迁移：{blocker}"),
            Style::default().fg(WARNING),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(
                Block::bordered()
                    .title(" 概览 ")
                    .border_style(Style::default().fg(ACCENT)),
            ),
        area,
    );
}

fn stat_line(label: &str, value: String, ok: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.to_string(), Style::default().fg(MUTED)),
        Span::styled("  ", Style::default()),
        Span::styled(
            value,
            Style::default().fg(if ok { Color::White } else { WARNING }),
        ),
    ])
}

fn draw_audit(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();

    for stage in [
        Stage::Scan,
        Stage::Backup,
        Stage::Database,
        Stage::Sessions,
        Stage::Verify,
    ] {
        let state = app.stage_state(stage);
        let (symbol, color) = match state {
            StageState::Pending => ("  .", MUTED),
            StageState::Active => ("  >", ACCENT),
            StageState::Done => ("  +", SUCCESS),
        };

        let mut spans = vec![
            Span::styled(symbol, Style::default().fg(color)),
            Span::styled(
                format!("  {}", stage.title()),
                Style::default().fg(if state == StageState::Pending {
                    MUTED
                } else {
                    Color::White
                }),
            ),
        ];

        // 当前阶段的实时细节显示在同一行。
        if state == StageState::Active
            && let Some(progress) = app.log.last()
            && progress.stage == stage
            && let Some(detail) = &progress.detail
        {
            spans.push(Span::styled(
                format!("   {detail}"),
                Style::default().fg(MUTED),
            ));
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .title(" 迁移审计 ")
                    .border_style(Style::default().fg(ACCENT)),
            ),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints = match app.overlay {
        Some(Overlay::Help)
        | Some(Overlay::Blocked)
        | Some(Overlay::Report)
        | Some(Overlay::Error)
        | Some(Overlay::Edit(_)) => " esc 关闭",
        Some(Overlay::Confirm) => " y 确认   esc 取消",
        None if app.busy == Busy::Migrating => " 正在迁移，请稍候",
        None => " 空格 勾选   a 全选   n 清空   t 目标   p 目录   m 迁移   r 重扫   ? 帮助   q 退出",
    };

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints.to_string(),
            Style::default().fg(MUTED),
        ))),
        area,
    );
}

// ------------------------------------------------------------------ 弹窗

fn popup(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let [area] = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// 按内容行数决定高度，保证矮终端下也尽量把内容完整显示出来。
fn popup_sized(area: Rect, width: u16, content_lines: usize) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(20);
    let height = (content_lines as u16 + 2).min(area.height.saturating_sub(2));
    let [row] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    cell
}

fn overlay_frame(title: &str, color: Color) -> Block<'static> {
    Block::bordered()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(color))
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let heading = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
    };

    // 控制在 14 行以内，这样 88x20 的矮终端也能完整读完。
    let text = vec![
        heading("为什么需要这个工具"),
        Line::from("Codex 只显示「历史记录里的 Provider」与 config.toml 当前 Provider 一致的历史会话；"),
        Line::from("切换 Provider 后文件仍在磁盘上，但 Codex 界面不再展示它们。"),
        heading("会改动什么"),
        Line::from("  state_5.sqlite    threads.model_provider（单事务更新）"),
        Line::from("  sessions/*.jsonl  session_meta 里的 payload.model_provider（只改这一行）"),
        heading("安全措施"),
        Line::from("  - 检测到 Codex 进程运行中、或 session 文件被占用时，拒绝写入"),
        Line::from("  - 写入前先创建完整备份：<Codex 目录>/backups/"),
        Line::from("  - 不联网、无遥测、不读取账号凭据"),
        heading("快捷键"),
        Line::from("  空格 勾选    a/n 全选    j k ↑ ↓ 移动光标"),
        Line::from("  t/p 改目标或目录    m 迁移    r 重扫    q 退出"),
        Line::from(Span::styled("按 esc 关闭", Style::default().fg(MUTED))),
    ];

    let area = popup_sized(area, 88, text.len());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(overlay_frame("帮助", ACCENT)),
        area,
    );
}

fn draw_confirm(frame: &mut Frame, area: Rect, app: &App) {
    let area = popup(area, 76, 62);
    frame.render_widget(Clear, area);

    let (database_rows, session_files) = app.pending_counts();
    let sources = app.selected_sources();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("把这些历史记录的 Provider 从 ", Style::default().fg(Color::White)),
            Span::styled(
                sources.join(", "),
                Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("改为 ", Style::default().fg(Color::White)),
            Span::styled(
                app.target_provider.clone(),
                Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(format!("  {database_rows} 条线程记录（state_5.sqlite）")),
        Line::from(format!("  {session_files} 个会话文件（sessions/）")),
        Line::from(""),
        Line::from(Span::styled(
            "开始之前会先创建带时间戳的完整备份，备份失败则不会写入。",
            Style::default().fg(MUTED),
        )),
    ];

    if let Some(scan) = &app.scan {
        lines.push(Line::from(Span::styled(
            format!("备份位置：{}/backups", scan.codex_home.display()),
            Style::default().fg(MUTED),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("y", Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)),
        Span::styled(" 开始迁移      ", Style::default().fg(Color::White)),
        Span::styled("esc", Style::default().fg(WARNING).add_modifier(Modifier::BOLD)),
        Span::styled(" 取消", Style::default().fg(Color::White)),
    ]));

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(overlay_frame("确认迁移", WARNING)),
        area,
    );
}

fn draw_blocked(frame: &mut Frame, area: Rect, app: &App) {
    let area = popup(area, 78, 62);
    frame.render_widget(Clear, area);

    let mut lines = vec![
        Line::from(Span::styled(
            "Codex 正在运行，数据库与 session 文件可能正被使用。",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from("请关闭所有 Codex CLI 与 Codex App 实例，然后重新检测。"),
        Line::from(""),
        Line::from(Span::styled(
            "检测到的进程",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
    ];

    for process in &app.blocked {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:>7}  ", process.pid), Style::default().fg(MUTED)),
            Span::styled(process.name.clone(), Style::default().fg(Color::White)),
            Span::styled(
                process
                    .path
                    .as_ref()
                    .map(|path| format!("   {path}"))
                    .unwrap_or_default(),
                Style::default().fg(MUTED),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("r", Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)),
        Span::styled(
            " 重新检测并迁移      ",
            Style::default().fg(Color::White),
        ),
        Span::styled("esc", Style::default().fg(WARNING).add_modifier(Modifier::BOLD)),
        Span::styled(" 关闭", Style::default().fg(Color::White)),
    ]));

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(overlay_frame("无法迁移：Codex 正在运行", Color::Red)),
        area,
    );
}

fn draw_report(frame: &mut Frame, area: Rect, app: &App) {
    let Some(outcome) = &app.outcome else {
        return;
    };

    let headline = if outcome.session_errors.is_empty() {
        Span::styled(
            "迁移完成",
            Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("迁移完成，但有 {} 个文件出错", outcome.session_errors.len()),
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
        )
    };

    let mut lines = vec![
        Line::from(headline),
        Line::from(""),
        Line::from(vec![
            Span::styled("Provider   ", Style::default().fg(MUTED)),
            Span::styled(
                format!(
                    "{}  ->  {}",
                    outcome.source_providers.join(", "),
                    outcome.target_provider
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("数据库     ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{} 条线程记录已更新", outcome.updated_thread_rows),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("Session    ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{} 个文件已更新", outcome.updated_session_files),
                Style::default().fg(Color::White),
            ),
        ]),
        // 迁移前后的 Provider 分布，才是「历史是否恢复可见」的直接证据。
        Line::from(vec![
            Span::styled("state_5.sqlite  ", Style::default().fg(MUTED)),
            Span::styled(
                format!(
                    "{}  ->  {}",
                    summarize(&outcome.before.database_distribution),
                    summarize(&outcome.after.database_distribution)
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("sessions/       ", Style::default().fg(MUTED)),
            Span::styled(
                format!(
                    "{}  ->  {}",
                    summarize(&outcome.before.session_distribution),
                    summarize(&outcome.after.session_distribution)
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "备份",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("  {}", outcome.backup_dir.to_string_lossy()),
            Style::default().fg(Color::White),
        )),
    ];

    for error in outcome.session_errors.iter().take(4) {
        lines.push(Line::from(Span::styled(
            format!("  ! {error}"),
            Style::default().fg(WARNING),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "重新打开 Codex 确认历史会话已恢复显示。按 esc 关闭。",
        Style::default().fg(MUTED),
    )));

    let area = popup_sized(area, 86, lines.len());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(overlay_frame("迁移结果", SUCCESS)),
        area,
    );
}

fn draw_error(frame: &mut Frame, area: Rect, app: &App) {
    let area = popup(area, 74, 42);
    frame.render_widget(Clear, area);

    let lines = vec![
        Line::from(Span::styled(
            app.error.clone().unwrap_or_else(|| "未知错误".to_string()),
            Style::default().fg(Color::Red),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "没有写入任何内容。按 esc 关闭。",
            Style::default().fg(MUTED),
        )),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(overlay_frame("迁移失败", Color::Red)),
        area,
    );
}

fn draw_edit(frame: &mut Frame, area: Rect, app: &App, field: EditField) {
    let area = popup(area, 78, 32);
    frame.render_widget(Clear, area);

    let (title, help) = match field {
        EditField::CodexHome => (
            "修改 Codex 目录",
            "填 .codex 目录的完整路径；留空则恢复为默认目录。",
        ),
        EditField::TargetProvider => (
            "修改目标 Provider",
            "填当前 config.toml 里 model_provider 的值。",
        ),
    };

    let lines = vec![
        Line::from(Span::styled(help, Style::default().fg(MUTED))),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(ACCENT)),
            Span::styled(
                app.edit_buffer.clone(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled("_", Style::default().fg(ACCENT)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "enter 确认      esc 取消",
            Style::default().fg(MUTED),
        )),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(overlay_frame(title, ACCENT)),
        area,
    );
}

/// `openai 1200, codex 36`：结果页里的迁移前后对比。
fn summarize(distribution: &ProviderDistribution) -> String {
    if distribution.is_empty() {
        return "无".to_string();
    }
    distribution
        .iter()
        .map(|(name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 把缓冲区转成文本，供快照测试断言。
///
/// 一个汉字占两个单元格，ratatui 会把右侧那一格重置掉；这里按单元格真实宽度前进，
/// 跳过后面的占位格，这样 dump 出来的内容和终端上看到的排版一致。
#[cfg(test)]
pub fn render_to_string(app: &App, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| draw(frame, app))
        .expect("render must not fail");

    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    let mut output = String::new();

    for y in area.y..area.bottom() {
        let mut x = area.x;
        while x < area.right() {
            let Some(cell) = buffer.cell((x, y)) else {
                break;
            };
            let symbol = cell.symbol();
            output.push_str(symbol);
            x += Span::raw(symbol).width().max(1) as u16;
        }
        output.push('\n');
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Key, WorkerEvent, fixture_scan};
    use crate::core::types::{MigrateError, MigrationOutcome, Progress, RunningProcess};
    use std::path::PathBuf;

    fn app() -> App {
        let mut app = App::for_test(r"C:\Users\me\.codex");
        app.apply_scan_for_test(fixture_scan(
            r"C:\Users\me\.codex",
            &[("openai", 1200), ("ccswitch", 5)],
            &[("openai", 400), ("codex", 36)],
            Some("ccswitch"),
        ));
        app
    }

    #[test]
    fn renders_header_target_and_provider_rows() {
        let text = render_to_string(&app(), 120, 40);

        assert!(text.contains("Codex 历史 Provider 迁移工具"), "{text}");
        assert!(text.contains(r"C:\Users\me\.codex"), "{text}");
        assert!(text.contains("ccswitch"), "{text}");
        assert!(text.contains("openai"), "{text}");
        assert!(text.contains("目标"), "{text}");
        assert!(text.contains("1200"), "{text}");
        assert!(text.contains("400"), "{text}");
    }

    #[test]
    fn renders_the_pending_totals() {
        let text = render_to_string(&app(), 160, 40);
        assert!(
            text.contains("1200 条线程记录、436 个会话文件"),
            "{text}"
        );
    }

    #[test]
    fn renders_the_unreadable_sessions_warning() {
        let mut app = App::for_test("x");
        let mut scan = fixture_scan("x", &[("openai", 1)], &[("openai", 1)], Some("ccswitch"));
        scan.unreadable_sessions = vec!["无法读取 x/bad.jsonl".to_string()];
        app.apply_scan_for_test(scan);

        let text = render_to_string(&app, 160, 36);
        assert!(text.contains("! 1 个 session"), "{text}");
        assert!(text.contains("无法读取"), "{text}");
    }

    #[test]
    fn renders_the_audit_checklist() {
        let mut app = app();
        app.apply_event_for_test(WorkerEvent::Progress(Box::new(Progress::new(
            Stage::Backup,
            "创建完整备份",
            Some("state_5.sqlite".to_string()),
        ))));

        let text = render_to_string(&app, 120, 40);
        assert!(text.contains("扫描配置"), "{text}");
        assert!(text.contains("完整备份"), "{text}");
        assert!(text.contains("更新 Session Metadata"), "{text}");
        // 扫描已完成，备份是当前阶段。
        assert!(text.contains("+  扫描配置"), "{text}");
        assert!(text.contains(">  完整备份"), "{text}");
    }

    #[test]
    fn renders_the_help_overlay() {
        let mut app = app();
        app.handle_key(Key::Char('?'));
        let text = render_to_string(&app, 120, 44);

        assert!(text.contains("为什么需要这个工具"), "{text}");
        assert!(text.contains("session_meta"), "{text}");
        assert!(text.contains("state_5.sqlite"), "{text}");
        assert!(text.contains("按 esc 关闭"), "{text}");
    }

    #[test]
    fn renders_the_confirm_overlay_with_the_selection() {
        let mut app = app();
        app.handle_key(Key::Char('m'));
        let text = render_to_string(&app, 120, 44);

        assert!(text.contains("确认迁移"), "{text}");
        assert!(text.contains("codex, openai"), "{text}");
        assert!(text.contains("改为 ccswitch"), "{text}");
        assert!(text.contains("1200 条线程记录"), "{text}");
        assert!(text.contains("436 个会话文件"), "{text}");
    }

    #[test]
    fn renders_the_blocked_overlay_with_process_details() {
        let mut app = app();
        app.apply_event_for_test(WorkerEvent::MigrationFailed(Box::new(
            MigrateError::CodexRunning(vec![RunningProcess {
                pid: 4242,
                name: "codex.exe".to_string(),
                path: Some(r"C:\bin\codex.exe".to_string()),
            }]),
        )));

        let text = render_to_string(&app, 120, 44);
        assert!(text.contains("Codex 正在运行"), "{text}");
        assert!(text.contains("4242"), "{text}");
        assert!(text.contains(r"C:\bin\codex.exe"), "{text}");
    }

    #[test]
    fn renders_the_result_overlay_with_the_backup_path() {
        let mut app = app();
        let before = fixture_scan(r"C:\Users\me\.codex", &[("openai", 1200)], &[], None);
        let after = fixture_scan(r"C:\Users\me\.codex", &[("ccswitch", 1205)], &[], None);
        app.apply_event_for_test(WorkerEvent::MigrationFinished(Box::new(MigrationOutcome {
            target_provider: "ccswitch".to_string(),
            source_providers: vec!["openai".to_string()],
            backup_dir: PathBuf::from(
                r"C:\Users\me\.codex\backups\provider-migration-20260101-120000",
            ),
            updated_thread_rows: 1200,
            updated_session_files: 400,
            session_errors: Vec::new(),
            before,
            after,
        })));

        let text = render_to_string(&app, 120, 44);
        assert!(text.contains("迁移完成"), "{text}");
        assert!(text.contains("provider-migration-20260101-120000"), "{text}");
        assert!(text.contains("1200 条线程记录已更新"), "{text}");
        // 迁移前后的 Provider 分布对比。
        assert!(text.contains("openai 1200  ->  ccswitch 1205"), "{text}");
    }

    #[test]
    fn renders_the_error_overlay() {
        let mut app = app();
        app.apply_event_for_test(WorkerEvent::MigrationFailed(Box::new(MigrateError::Failed(
            "未找到 Codex 状态数据库".to_string(),
        ))));

        let text = render_to_string(&app, 120, 44);
        assert!(text.contains("迁移失败"), "{text}");
        assert!(text.contains("未找到 Codex 状态数据库"), "{text}");
        assert!(text.contains("没有写入任何内容"), "{text}");
    }

    #[test]
    fn renders_the_edit_overlay() {
        let mut app = app();
        app.begin_edit(EditField::TargetProvider);
        let text = render_to_string(&app, 120, 44);

        assert!(text.contains("修改目标 Provider"), "{text}");
        assert!(text.contains("enter 确认"), "{text}");
    }

    #[test]
    fn a_tiny_terminal_degrades_instead_of_panicking() {
        let text = render_to_string(&app(), 40, 10);
        assert!(text.contains("终端窗口太小"), "{text}");
        assert!(text.contains("至少需要 96x20"), "{text}");
    }

    #[test]
    fn an_empty_scan_renders_without_panicking() {
        let app = App::for_test("x");
        let text = render_to_string(&app, 100, 30);
        assert!(text.contains("正在扫描 Codex 目录"), "{text}");
    }

    #[test]
    fn every_stage_state_renders() {
        let mut app = app();
        app.busy = Busy::Migrating;
        for stage in [
            Stage::Scan,
            Stage::Backup,
            Stage::Database,
            Stage::Sessions,
            Stage::Verify,
            Stage::Done,
        ] {
            app.apply_event_for_test(WorkerEvent::Progress(Box::new(Progress::new(
                stage,
                "处理中",
                Some("细节".to_string()),
            ))));
            let text = render_to_string(&app, 120, 40);
            assert!(text.contains("迁移审计"), "{text}");
        }
    }

    /// 汉字是双宽字符，必须确认它们没有把边框挤歪。
    #[test]
    fn wide_characters_keep_the_frame_aligned() {
        let text = render_to_string(&app(), 120, 40);
        if std::env::var_os("SHOW_TUI").is_some() {
            println!("\n{text}");
        }

        for line in text.lines() {
            let width: usize = Span::raw(line).width();
            assert_eq!(
                width, 120,
                "line is not exactly 120 columns wide: {line:?}"
            );
        }
    }

    /// 每个弹窗都必须在最小可用尺寸下渲染出来，且不越界、不 panic。
    ///
    /// 设置环境变量 `SHOW_TUI=1` 运行本测试（`-- --nocapture`）可以把每个界面打印出来预览。
    #[test]
    fn every_overlay_renders_at_the_minimum_size() {
        let cases: Vec<(&str, App)> = vec![
            ("主界面", app()),
            ("帮助", {
                let mut app = app();
                app.handle_key(Key::Char('?'));
                app
            }),
            ("确认迁移", {
                let mut app = app();
                app.handle_key(Key::Char('m'));
                app
            }),
            ("进程占用", {
                let mut app = app();
                app.apply_event_for_test(WorkerEvent::MigrationFailed(Box::new(
                    MigrateError::CodexRunning(vec![RunningProcess {
                        pid: 4242,
                        name: "codex.exe".to_string(),
                        path: Some(r"C:\Program Files\OpenAI\codex.exe".to_string()),
                    }]),
                )));
                app
            }),
            ("迁移结果", {
                let mut app = app();
                app.apply_event_for_test(WorkerEvent::MigrationFinished(Box::new(
                    MigrationOutcome {
                        target_provider: "ccswitch".to_string(),
                        source_providers: vec!["codex".to_string(), "openai".to_string()],
                        backup_dir: PathBuf::from(
                            r"C:\Users\me\.codex\backups\provider-migration-20260101-120000",
                        ),
                        updated_thread_rows: 1200,
                        updated_session_files: 436,
                        session_errors: Vec::new(),
                        before: fixture_scan(
                            r"C:\Users\me\.codex",
                            &[("openai", 1200)],
                            &[("openai", 400), ("codex", 36)],
                            None,
                        ),
                        after: fixture_scan(
                            r"C:\Users\me\.codex",
                            &[("ccswitch", 1205)],
                            &[("ccswitch", 436)],
                            None,
                        ),
                    },
                )));
                app
            }),
            ("迁移失败", {
                let mut app = app();
                app.apply_event_for_test(WorkerEvent::MigrationFailed(Box::new(
                    MigrateError::Failed("未找到 Codex 状态数据库".to_string()),
                )));
                app
            }),
            ("修改目录", {
                let mut app = app();
                app.begin_edit(EditField::CodexHome);
                app
            }),
            ("修改目标", {
                let mut app = app();
                app.begin_edit(EditField::TargetProvider);
                app
            }),
        ];

        for (name, app) in cases {
            let text = render_to_string(&app, MIN_WIDTH, MIN_HEIGHT);
            assert_eq!(
                text.lines().count(),
                MIN_HEIGHT as usize,
                "{name} did not fill the frame"
            );
            for line in text.lines() {
                assert_eq!(
                    Span::raw(line).width(),
                    MIN_WIDTH as usize,
                    "{name} overflows the frame: {line:?}"
                );
            }
            if std::env::var_os("SHOW_TUI").is_some() {
                println!("\n===== {name} ({MIN_WIDTH}x{MIN_HEIGHT}) =====\n{text}");
            }
        }
    }

    /// 每一行的边框必须完整：正文区的文字如果溢出，就会盖住右栏的右边框。
    ///
    /// 这个测试专门盯着「内容比栏位宽」这类问题——只比较总宽度是发现不了的，
    /// 因为溢出的文字只是把边框那一格覆盖掉了。
    #[test]
    fn panel_borders_are_never_overwritten() {
        for (width, height) in [(MIN_WIDTH, MIN_HEIGHT), (120, 40), (200, 50)] {
            let text = render_to_string(&app(), width, height);
            let lines: Vec<&str> = text.lines().collect();

            // 正文区：从表头之后到审计面板之前，每行都应以外框的竖线收尾。
            // 最后一行是底部快捷键提示，本身没有边框，跳过。
            for (index, line) in lines.iter().enumerate().take(lines.len().saturating_sub(1)) {
                assert!(
                    line.ends_with('│') || line.ends_with('┐') || line.ends_with('┘'),
                    "{width}x{height} 第 {index} 行右边界被覆盖: {line:?}"
                );
            }
        }
    }

    /// 弹窗内容不能被裁掉：矮终端下也必须能看到最后一个区块。
    #[test]
    fn overlays_are_not_clipped_at_the_minimum_size() {
        let mut help = app();
        help.handle_key(Key::Char('?'));
        let text = render_to_string(&help, MIN_WIDTH, MIN_HEIGHT);
        // help 的四个区块标题依次出现，说明整篇都渲染出来了。
        for section in ["为什么需要这个工具", "会改动什么", "安全措施", "快捷键", "按 esc 关闭"] {
            assert!(text.contains(section), "help is clipped before {section}: {text}");
        }

        let mut report = app();
        report.apply_event_for_test(WorkerEvent::MigrationFinished(Box::new(MigrationOutcome {
            target_provider: "ccswitch".to_string(),
            source_providers: vec!["openai".to_string()],
            backup_dir: PathBuf::from(
                r"C:\Users\me\.codex\backups\provider-migration-20260101-120000",
            ),
            updated_thread_rows: 1200,
            updated_session_files: 400,
            session_errors: Vec::new(),
            before: fixture_scan(r"C:\Users\me\.codex", &[("openai", 1200)], &[], None),
            after: fixture_scan(r"C:\Users\me\.codex", &[("ccswitch", 1205)], &[], None),
        })));
        let text = render_to_string(&report, MIN_WIDTH, MIN_HEIGHT);
        // 含备份路径与最后一行提示。
        assert!(
            text.contains("provider-migration-20260101-120000"),
            "report is clipped before the backup path: {text}"
        );
        assert!(
            text.contains("重新打开 Codex 确认历史会话已恢复显示"),
            "report is clipped before the closing hint: {text}"
        );
    }
}
