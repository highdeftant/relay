//! TUI dashboard — Ratatui + crossterm event loop.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
};

use crate::{
    app::{AppState, Tab},
    avatar,
    config::AppConfig,
    protocol, storage,
};

// Muted, lighter dark theme (no pure black)
const COLOR_VOID: Color = Color::Rgb(24, 28, 34);
const COLOR_STEEL: Color = Color::Rgb(72, 82, 96);
const COLOR_GHOST: Color = Color::Rgb(214, 221, 230);
const COLOR_GREEN: Color = Color::Rgb(134, 239, 172);
const COLOR_CYAN: Color = Color::Rgb(147, 197, 253);
const COLOR_AMBER: Color = Color::Rgb(252, 211, 77);
const COLOR_ORANGE: Color = Color::Rgb(251, 146, 60);
const COLOR_RED: Color = Color::Rgb(248, 113, 113);

enum UiAction {
    SendChatMessage,
}

/// Entry point for `relay watch`.
pub async fn watch(config: AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnterAlternateScreen)?;

    let result = run_dashboard(&mut terminal, config).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    ratatui::restore();

    result
}

async fn run_dashboard(terminal: &mut DefaultTerminal, config: AppConfig) -> Result<()> {
    let mut state = AppState::new();
    refresh_state(&config, &mut state);

    let mut last_refresh = Instant::now();

    loop {
        terminal.draw(|frame| draw(frame, &state))?;

        if last_refresh.elapsed() >= Duration::from_millis(900) {
            refresh_state(&config, &mut state);
            last_refresh = Instant::now();
        }

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if let Some(action) = handle_key(&mut state, key.code, key.modifiers) {
                match action {
                    UiAction::SendChatMessage => {
                        let msg = state.chat_input.trim().to_string();
                        if !msg.is_empty() {
                            match protocol::send_message_quiet(
                                &config,
                                &state.chat_agent,
                                &state.active_channel,
                                &msg,
                            )
                            .await
                            {
                                Ok(()) => {
                                    state.chat_input.clear();
                                    refresh_state(&config, &mut state);
                                }
                                Err(error) => {
                                    state.logs.push(format!("send_message failed: {error}"));
                                    if state.logs.len() > 200 {
                                        let keep_from = state.logs.len().saturating_sub(200);
                                        state.logs.drain(0..keep_from);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if state.should_quit {
            break;
        }
    }

    Ok(())
}

fn refresh_state(config: &AppConfig, state: &mut AppState) {
    match storage::load_agents(config) {
        Ok(map) => {
            let mut rows = map.into_values().collect::<Vec<_>>();
            rows.sort_by(|a, b| a.name.cmp(&b.name));
            state.agents = rows;
            state.clamp_selection();
        }
        Err(error) => {
            state.logs.push(format!("load_agents failed: {error}"));
        }
    }

    match storage::load_channel_events(config, &state.active_channel, 200) {
        Ok(events) => state.messages = events,
        Err(error) => state.logs.push(format!(
            "load_channel_events failed for {}: {error}",
            state.active_channel
        )),
    }

    if state.logs.len() > 200 {
        let keep_from = state.logs.len().saturating_sub(200);
        state.logs.drain(0..keep_from);
    }
}

fn handle_key(state: &mut AppState, code: KeyCode, modifiers: KeyModifiers) -> Option<UiAction> {
    match code {
        KeyCode::Char('q') if !state.active_tab.eq(&Tab::Chat) => {
            state.should_quit = true;
            return None;
        }
        KeyCode::Esc => {
            state.should_quit = true;
            return None;
        }
        KeyCode::Tab => {
            state.active_tab = state.active_tab.next();
            return None;
        }
        KeyCode::BackTab => {
            state.active_tab = state.active_tab.prev();
            return None;
        }
        KeyCode::Char('1') => {
            state.active_tab = Tab::Chat;
            return None;
        }
        KeyCode::Char('2') => {
            state.active_tab = Tab::Agents;
            return None;
        }
        KeyCode::Char('3') => {
            state.active_tab = Tab::Files;
            return None;
        }
        KeyCode::Char('4') => {
            state.active_tab = Tab::Logs;
            return None;
        }
        KeyCode::Char('5') => {
            state.active_tab = Tab::Activity;
            return None;
        }
        KeyCode::Char('6') => {
            state.active_tab = Tab::System;
            return None;
        }
        _ => {}
    }

    match state.active_tab {
        Tab::Agents => match code {
            KeyCode::Down | KeyCode::Char('j') => state.select_next_agent(),
            KeyCode::Up | KeyCode::Char('k') => state.select_prev_agent(),
            _ => {}
        },
        Tab::Chat => match code {
            KeyCode::Enter => return Some(UiAction::SendChatMessage),
            KeyCode::Backspace => {
                state.chat_input.pop();
            }
            KeyCode::Char('q') if state.chat_input.is_empty() => {
                state.should_quit = true;
            }
            KeyCode::Char(c) => {
                if !modifiers.contains(KeyModifiers::CONTROL) {
                    state.chat_input.push(c);
                }
            }
            _ => {}
        },
        Tab::Files | Tab::Logs | Tab::Activity | Tab::System => {}
    }

    None
}

fn draw(frame: &mut Frame<'_>, state: &AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_title(frame, outer[0], state);
    draw_tabs(frame, outer[1], state.active_tab);
    draw_content(frame, outer[2], state);
    draw_status(frame, outer[3], state);
}

fn draw_title(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " 同期//SYNC ",
            Style::default()
                .fg(COLOR_GREEN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("RELAY v0.1", Style::default().fg(COLOR_GHOST)),
        Span::styled("  ──  ", Style::default().fg(COLOR_STEEL)),
        Span::styled("AGENTS: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", state.agents.len()),
            Style::default().fg(COLOR_GREEN),
        ),
        Span::styled("  |  ", Style::default().fg(COLOR_STEEL)),
        Span::styled("CH: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("#{}", state.active_channel),
            Style::default().fg(COLOR_AMBER),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(COLOR_STEEL)),
    );

    frame.render_widget(title, area);
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect, active: Tab) {
    let titles: Vec<Line<'_>> = Tab::ALL
        .iter()
        .map(|t| {
            let is_active = *t == active;
            let prefix = if is_active { "> " } else { "  " };
            let style = if is_active {
                Style::default()
                    .fg(COLOR_GREEN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_GHOST)
            };
            Line::from(vec![Span::styled(format!("{prefix}{}", t.label()), style)])
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(COLOR_CYAN)),
        )
        .style(Style::default().bg(COLOR_VOID))
        .divider(Span::styled("  │  ", Style::default().fg(COLOR_STEEL)));

    frame.render_widget(tabs, area);
}

fn draw_content(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    match state.active_tab {
        Tab::Chat => draw_chat_panel(frame, area, state),
        Tab::Agents => draw_agents_panel(frame, area, state),
        Tab::Files => draw_placeholder(frame, area, " ファイル//FILES ", "[files tab in progress]"),
        Tab::Logs => draw_logs_panel(frame, area, state),
        Tab::Activity => draw_activity_panel(frame, area, state),
        Tab::System => draw_system_panel(frame, area, state),
    }
}

fn draw_chat_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let mut lines = vec![Line::from("")];
    let feed_height = split[0].height.saturating_sub(3) as usize;

    let start = state.messages.len().saturating_sub(feed_height.max(1));
    for msg in state.messages.iter().skip(start) {
        let ts = human_ts(&msg.timestamp);
        lines.push(Line::from(vec![
            Span::styled(format!(" [{ts}] "), Style::default().fg(COLOR_STEEL)),
            Span::styled(format!("{}", msg.agent), Style::default().fg(COLOR_CYAN)),
            Span::styled(": ", Style::default().fg(COLOR_GHOST)),
            Span::styled(truncate(&msg.message, 72), Style::default().fg(COLOR_GHOST)),
        ]));
    }

    if state.messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no messages yet",
            Style::default().fg(COLOR_STEEL),
        )));
    }

    let feed = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                format!(" #{} ", state.active_channel),
                Style::default().fg(COLOR_CYAN),
            )),
    );

    frame.render_widget(feed, split[0]);

    let input = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} > ", state.chat_agent),
            Style::default().fg(COLOR_AMBER),
        ),
        Span::styled(
            format!("{}█", state.chat_input),
            Style::default().fg(COLOR_GHOST),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_GREEN))
            .title(Span::styled(
                " INPUT (Enter to send) ",
                Style::default().fg(COLOR_GREEN),
            )),
    );
    frame.render_widget(input, split[1]);
}

fn draw_logs_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines = vec![Line::from("")];
    let cap = area.height.saturating_sub(3) as usize;
    let start = state.logs.len().saturating_sub(cap.max(1));

    for log in state.logs.iter().skip(start) {
        lines.push(Line::from(Span::styled(
            format!("  {}", truncate(log, 100)),
            Style::default().fg(COLOR_STEEL),
        )));
    }

    if state.logs.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no logs",
            Style::default().fg(COLOR_STEEL),
        )));
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                " ログ//LOGS ",
                Style::default().fg(COLOR_CYAN),
            )),
    );

    frame.render_widget(widget, area);
}

fn draw_activity_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines = vec![Line::from("")];
    let cap = area.height.saturating_sub(4) as usize;

    let msg_count = cap.min(state.messages.len());
    let log_count = cap.saturating_sub(msg_count).min(state.logs.len());

    if msg_count == 0 && log_count == 0 {
        lines.push(Line::from(Span::styled(
            "  no recent activity",
            Style::default().fg(COLOR_STEEL),
        )));
    }

    for msg in state.messages.iter().rev().take(msg_count).rev() {
        lines.push(Line::from(vec![
            Span::styled("  MSG ", Style::default().fg(COLOR_AMBER)),
            Span::styled(format!("{}", msg.agent), Style::default().fg(COLOR_CYAN)),
            Span::styled(": ", Style::default().fg(COLOR_GHOST)),
            Span::styled(truncate(&msg.message, 72), Style::default().fg(COLOR_GHOST)),
        ]));
    }

    for log in state.logs.iter().rev().take(log_count).rev() {
        lines.push(Line::from(vec![
            Span::styled("  LOG ", Style::default().fg(COLOR_RED)),
            Span::styled(truncate(log, 82), Style::default().fg(COLOR_STEEL)),
        ]));
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                " ACTIVITY//FEED ",
                Style::default().fg(COLOR_CYAN),
            )),
    );

    frame.render_widget(widget, area);
}

fn draw_system_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let online = state
        .agents
        .iter()
        .filter(|a| a.status != "offline")
        .count();
    let working = state
        .agents
        .iter()
        .filter(|a| a.status == "working")
        .count();

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Build: ", Style::default().fg(COLOR_GHOST)),
            Span::styled("relay v0.1", Style::default().fg(COLOR_AMBER)),
        ]),
        Line::from(vec![
            Span::styled("  Tabs: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                "CHAT AGENTS FILES LOGS ACTIVITY SYSTEM",
                Style::default().fg(COLOR_CYAN),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Agents total: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", state.agents.len()),
                Style::default().fg(COLOR_GREEN),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Online: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(format!("{}", online), Style::default().fg(COLOR_GREEN)),
            Span::styled("  Working: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(format!("{}", working), Style::default().fg(COLOR_AMBER)),
        ]),
        Line::from(vec![
            Span::styled("  Active channel: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("#{}", state.active_channel),
                Style::default().fg(COLOR_CYAN),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Messages loaded: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", state.messages.len()),
                Style::default().fg(COLOR_CYAN),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Logs buffered: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", state.logs.len()),
                Style::default().fg(COLOR_STEEL),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  next: channel switching + skills/memory panes",
            Style::default().fg(COLOR_STEEL),
        )),
    ];

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                " SYSTEM//STATUS ",
                Style::default().fg(COLOR_CYAN),
            )),
    );

    frame.render_widget(widget, area);
}

fn draw_placeholder(frame: &mut Frame<'_>, area: Rect, title: &str, body: &str) {
    let widget = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {body}"),
            Style::default().fg(COLOR_STEEL),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                title.to_string(),
                Style::default().fg(COLOR_CYAN),
            )),
    );

    frame.render_widget(widget, area);
}

fn draw_agents_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    draw_agents_list(frame, split[0], state);
    draw_agent_detail(frame, split[1], state);
}

fn draw_agents_list(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines = vec![Line::from("")];

    if state.agents.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no agents connected",
            Style::default().fg(COLOR_STEEL),
        )));
    } else {
        for (idx, agent) in state.agents.iter().enumerate() {
            let selected = idx == state.selected_agent;
            let dot = if agent.status == "offline" {
                "○"
            } else {
                "●"
            };
            let status_color = status_color(&agent.status);
            let role = agent.role.as_deref().unwrap_or("-");
            let task = agent.task.as_deref().unwrap_or("");
            let marker = if selected { ">" } else { " " };

            let style = if selected {
                Style::default()
                    .fg(COLOR_GREEN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_GHOST)
            };

            lines.push(Line::from(vec![
                Span::styled(format!(" {marker} "), style),
                Span::styled(format!("{dot} "), Style::default().fg(status_color)),
                Span::styled(
                    format!("{:<10} [{:<10}] {:<8}", agent.name, role, agent.status),
                    style,
                ),
                Span::styled(
                    if task.is_empty() {
                        String::new()
                    } else {
                        format!(" :: {}", truncate(task, 20))
                    },
                    Style::default().fg(COLOR_STEEL),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));

    let online = state
        .agents
        .iter()
        .filter(|a| a.status != "offline")
        .count();
    let offline = state.agents.len().saturating_sub(online);
    let working = state
        .agents
        .iter()
        .filter(|a| a.status == "working")
        .count();

    lines.push(Line::from(vec![
        Span::styled(" AGENTS: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", state.agents.len()),
            Style::default().fg(COLOR_GREEN),
        ),
        Span::styled(" | ONLINE: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(format!("{:02}", online), Style::default().fg(COLOR_GREEN)),
        Span::styled(" | OFFLINE: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(format!("{:02}", offline), Style::default().fg(COLOR_STEEL)),
        Span::styled(" | WORKING: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(format!("{:02}", working), Style::default().fg(COLOR_AMBER)),
    ]));

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                " 接続//AGENTS ",
                Style::default().fg(COLOR_CYAN),
            )),
    );

    frame.render_widget(widget, area);
}

fn draw_agent_detail(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines = vec![Line::from("")];

    if let Some(agent) = state.selected_agent_ref() {
        let avatar = avatar::generate(&agent.name, None);
        lines.push(Line::from(Span::styled(
            format!("  {}", agent.name.to_uppercase()),
            Style::default()
                .fg(status_color(&agent.status))
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  role: {}", agent.role.as_deref().unwrap_or("-")),
            Style::default().fg(COLOR_GHOST),
        )));
        lines.push(Line::from(Span::styled(
            format!("  status: {}", agent.status),
            Style::default().fg(status_color(&agent.status)),
        )));
        lines.push(Line::from(Span::styled(
            format!("  last_seen_epoch: {}", agent.last_seen_epoch),
            Style::default().fg(COLOR_STEEL),
        )));
        if let Some(task) = &agent.task {
            lines.push(Line::from(Span::styled(
                format!("  task: {}", truncate(task, 26)),
                Style::default().fg(COLOR_AMBER),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  avatar",
            Style::default().fg(COLOR_CYAN).add_modifier(Modifier::BOLD),
        )));
        for row in &avatar.lines {
            lines.push(Line::from(Span::styled(
                format!("  {row}"),
                Style::default().fg(status_color(&agent.status)),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  select an agent",
            Style::default().fg(COLOR_STEEL),
        )));
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(" PROFILE ", Style::default().fg(COLOR_CYAN))),
    );

    frame.render_widget(widget, area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let chat_hint = if state.active_tab == Tab::Chat {
        "[Enter] Send "
    } else {
        ""
    };

    let status = Paragraph::new(Line::from(vec![
        Span::styled(" [Tab] Switch ", Style::default().fg(COLOR_STEEL)),
        Span::styled("[1-6] Tabs ", Style::default().fg(COLOR_STEEL)),
        Span::styled("[j/k] Select Agent ", Style::default().fg(COLOR_STEEL)),
        Span::styled(chat_hint, Style::default().fg(COLOR_STEEL)),
        Span::styled("[q] Quit ", Style::default().fg(COLOR_STEEL)),
        Span::styled("│", Style::default().fg(COLOR_STEEL)),
        Span::styled(
            format!(" {} ", state.active_tab.label()),
            Style::default().fg(COLOR_ORANGE),
        ),
    ]));

    frame.render_widget(status, area);
}

fn status_color(status: &str) -> Color {
    match status {
        "working" => COLOR_AMBER,
        "offline" => COLOR_RED,
        _ => COLOR_GREEN,
    }
}

fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        return input.to_string();
    }
    let mut out = input
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn human_ts(epoch: &str) -> String {
    let parsed = epoch.parse::<u64>().unwrap_or(0);
    let hhmmss = parsed % 86_400;
    let h = hhmmss / 3600;
    let m = (hhmmss % 3600) / 60;
    let s = hhmmss % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
