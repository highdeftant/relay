//! TUI dashboard — Ratatui + crossterm event loop.

use std::time::{Duration, Instant, SystemTime};

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
    hermes, protocol, storage,
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

const AGENT_STALE_SECS: u64 = 90;
const AGENT_OFFLINE_SECS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentHealth {
    Working,
    Online,
    Stale,
    Offline,
}

#[derive(Debug, Default, Clone, Copy)]
struct HealthCounts {
    working: usize,
    online: usize,
    stale: usize,
    offline: usize,
}

enum UiAction {
    SendChatMessage,
    NextChannel,
    PrevChannel,
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
                    UiAction::NextChannel => {
                        state.select_next_channel();
                        refresh_state(&config, &mut state);
                    }
                    UiAction::PrevChannel => {
                        state.select_prev_channel();
                        refresh_state(&config, &mut state);
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
    state.hermes_snapshot = hermes::load_snapshot();

    state.last_refresh_unix = now_unix_secs();

    match storage::load_agents(config) {
        Ok(map) => {
            let mut rows = map.into_values().collect::<Vec<_>>();
            sort_agents_by_health(&mut rows);
            state.agents = rows;
            state.clamp_selection();
        }
        Err(error) => {
            state.logs.push(format!("load_agents failed: {error}"));
        }
    }

    match storage::list_channels(config) {
        Ok(channels) => state.set_channels(channels),
        Err(error) => state.logs.push(format!("list_channels failed: {error}")),
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
            state.active_tab = Tab::Knowledge;
            return None;
        }
        KeyCode::Char('4') => {
            state.active_tab = Tab::Memory;
            return None;
        }
        KeyCode::Char('5') => {
            state.active_tab = Tab::System;
            return None;
        }
        _ => {}
    }

    match state.active_tab {
        Tab::Agents => match code {
            KeyCode::Down | KeyCode::Char('j') => state.select_next_agent(),
            KeyCode::Up | KeyCode::Char('k') => state.select_prev_agent(),
            KeyCode::Char('d') => state.open_dm_with_selected(),
            _ => {}
        },
        Tab::Chat => match code {
            KeyCode::Enter => return Some(UiAction::SendChatMessage),
            KeyCode::Char(']') if state.chat_input.is_empty() => {
                return Some(UiAction::NextChannel);
            }
            KeyCode::Char('[') if state.chat_input.is_empty() => {
                return Some(UiAction::PrevChannel);
            }
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
        Tab::Knowledge | Tab::Memory | Tab::System => {}
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
    let counts = health_counts(&state.agents);

    let mut spans = vec![
        Span::styled(
            " 同期//SYNC ",
            Style::default()
                .fg(COLOR_GREEN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("RELAY v0.1", Style::default().fg(COLOR_GHOST)),
        Span::styled("  ──  ", Style::default().fg(COLOR_STEEL)),
    ];

    // Health-first: always show agent counts
    spans.push(Span::styled("AGENTS: ", Style::default().fg(COLOR_GHOST)));
    spans.push(Span::styled(
        format!("{:02}", state.agents.len()),
        Style::default().fg(COLOR_GREEN),
    ));

    if counts.online > 0 {
        spans.push(Span::styled("  ", Style::default().fg(COLOR_STEEL)));
        spans.push(Span::styled("●", Style::default().fg(COLOR_GREEN)));
        spans.push(Span::styled(
            format!("{}", counts.online),
            Style::default().fg(COLOR_GREEN),
        ));
    }
    if counts.working > 0 {
        spans.push(Span::styled("  ", Style::default().fg(COLOR_STEEL)));
        spans.push(Span::styled("◆", Style::default().fg(COLOR_AMBER)));
        spans.push(Span::styled(
            format!("{}", counts.working),
            Style::default().fg(COLOR_AMBER),
        ));
    }
    if counts.stale > 0 {
        spans.push(Span::styled("  ", Style::default().fg(COLOR_STEEL)));
        spans.push(Span::styled("◐", Style::default().fg(COLOR_ORANGE)));
        spans.push(Span::styled(
            format!("{}", counts.stale),
            Style::default().fg(COLOR_ORANGE),
        ));
    }
    if counts.offline > 0 {
        spans.push(Span::styled("  ", Style::default().fg(COLOR_STEEL)));
        spans.push(Span::styled("○", Style::default().fg(COLOR_RED)));
        spans.push(Span::styled(
            format!("{}", counts.offline),
            Style::default().fg(COLOR_RED),
        ));
    }

    spans.push(Span::styled("  |  ", Style::default().fg(COLOR_STEEL)));
    spans.push(Span::styled("CH: ", Style::default().fg(COLOR_GHOST)));
    spans.push(Span::styled(
        format!("#{}", state.active_channel),
        Style::default().fg(COLOR_AMBER),
    ));

    let title = Paragraph::new(Line::from(spans)).block(
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
        Tab::Knowledge => draw_knowledge_panel(frame, area, state),
        Tab::Memory => draw_memory_panel(frame, area, state),
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

    let channels_line = state
        .channels
        .iter()
        .map(|channel| {
            if channel == &state.active_channel {
                format!("[{channel}]")
            } else {
                channel.clone()
            }
        })
        .collect::<Vec<String>>()
        .join(" ");

    let feed = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                format!(
                    " #{}  [{} / {}] ",
                    state.active_channel,
                    state.channels.len(),
                    channels_line
                ),
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

fn draw_knowledge_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let snapshot = &state.hermes_snapshot;

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(10)])
        .split(area);

    let mut lines = vec![Line::from("")];
    lines.push(Line::from(vec![
        Span::styled("  Skills: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{}", snapshot.skill_count),
            Style::default().fg(COLOR_GREEN),
        ),
        Span::styled("   Sessions: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{}", snapshot.session_count),
            Style::default().fg(COLOR_GREEN),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  categories",
        Style::default().fg(COLOR_CYAN).add_modifier(Modifier::BOLD),
    )));

    if snapshot.skill_categories.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none found",
            Style::default().fg(COLOR_STEEL),
        )));
    } else {
        for category in snapshot.skill_categories.iter() {
            lines.push(Line::from(vec![
                Span::styled("   - ", Style::default().fg(COLOR_STEEL)),
                Span::styled(category.clone(), Style::default().fg(COLOR_GHOST)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  profiles",
        Style::default().fg(COLOR_CYAN).add_modifier(Modifier::BOLD),
    )));

    if snapshot.profile_skill_counts.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none found",
            Style::default().fg(COLOR_STEEL),
        )));
    } else {
        let mut profiles: Vec<_> = snapshot.profile_skill_counts.iter().collect();
        profiles.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        for (name, count) in profiles.iter() {
            lines.push(Line::from(vec![
                Span::styled("   - ", Style::default().fg(COLOR_STEEL)),
                Span::styled(format!("{name}: "), Style::default().fg(COLOR_GHOST)),
                Span::styled(format!("{count}"), Style::default().fg(COLOR_AMBER)),
            ]));
        }
    }

    let top = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                " HERMES//KNOWLEDGE ",
                Style::default().fg(COLOR_CYAN),
            )),
    );
    frame.render_widget(top, split[0]);

    let mut recent_lines = vec![Line::from("")];
    recent_lines.push(Line::from(Span::styled(
        "  recent sessions",
        Style::default().fg(COLOR_CYAN).add_modifier(Modifier::BOLD),
    )));

    if snapshot.recent_sessions.is_empty() {
        recent_lines.push(Line::from(Span::styled(
            "  none found",
            Style::default().fg(COLOR_STEEL),
        )));
    } else {
        for session in snapshot.recent_sessions.iter() {
            let label = session
                .replace("session_", "")
                .replace(".json", "")
                .replace(".jsonl", "");
            recent_lines.push(Line::from(vec![
                Span::styled("   - ", Style::default().fg(COLOR_STEEL)),
                Span::styled(truncate(&label, 68), Style::default().fg(COLOR_GHOST)),
            ]));
        }
    }

    let bottom = Paragraph::new(recent_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(" SESSIONS ", Style::default().fg(COLOR_CYAN))),
    );
    frame.render_widget(bottom, split[1]);
}

fn draw_memory_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let snapshot = &state.hermes_snapshot;

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    // Top: memory files
    let mut mem_lines = vec![Line::from("")];
    let memories = read_memory_files();
    if memories.is_empty() {
        mem_lines.push(Line::from(Span::styled(
            "  no memory files found",
            Style::default().fg(COLOR_STEEL),
        )));
    } else {
        for (name, content) in &memories {
            let lines_count = content.lines().count();
            let preview = content.lines().next().unwrap_or("");
            mem_lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(COLOR_STEEL)),
                Span::styled(
                    format!("{name} "),
                    Style::default().fg(COLOR_CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("({lines_count} lines)"),
                    Style::default().fg(COLOR_STEEL),
                ),
            ]));
            mem_lines.push(Line::from(vec![
                Span::styled("    ", Style::default().fg(COLOR_STEEL)),
                Span::styled(truncate(preview, 72), Style::default().fg(COLOR_GHOST)),
            ]));
            mem_lines.push(Line::from(""));
        }
    }

    let top = Paragraph::new(mem_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                " MEMORY//FILES ",
                Style::default().fg(COLOR_CYAN),
            )),
    );
    frame.render_widget(top, split[0]);

    // Bottom: hermes runtime state
    let mut state_lines = vec![Line::from("")];
    state_lines.push(Line::from(vec![
        Span::styled("  state.db: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            if snapshot.state_db_exists {
                format!("present ({})", human_bytes(snapshot.state_db_bytes))
            } else {
                "missing".to_string()
            },
            Style::default().fg(if snapshot.state_db_exists {
                COLOR_GREEN
            } else {
                COLOR_RED
            }),
        ),
    ]));
    state_lines.push(Line::from(vec![
        Span::styled("  config.yaml: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            if snapshot.config_exists {
                "present"
            } else {
                "missing"
            },
            Style::default().fg(if snapshot.config_exists {
                COLOR_GREEN
            } else {
                COLOR_RED
            }),
        ),
    ]));
    state_lines.push(Line::from(vec![
        Span::styled("  auth.json: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            if snapshot.auth_exists {
                "present"
            } else {
                "missing"
            },
            Style::default().fg(if snapshot.auth_exists {
                COLOR_GREEN
            } else {
                COLOR_RED
            }),
        ),
    ]));
    state_lines.push(Line::from(vec![
        Span::styled("  honcho hosts: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{}", snapshot.honcho_hosts),
            Style::default().fg(COLOR_CYAN),
        ),
    ]));
    state_lines.push(Line::from(vec![
        Span::styled("  processes: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            if snapshot.processes_file_exists {
                format!("{} known", snapshot.known_process_count)
            } else {
                "missing".to_string()
            },
            Style::default().fg(COLOR_GHOST),
        ),
    ]));

    let bottom = Paragraph::new(state_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(
                " RUNTIME//STATE ",
                Style::default().fg(COLOR_CYAN),
            )),
    );
    frame.render_widget(bottom, split[1]);
}

fn draw_system_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let counts = health_counts(&state.agents);

    let refresh_age = if state.last_refresh_unix > 0 {
        human_age_short(state.last_refresh_unix)
    } else {
        "-".to_string()
    };

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Build: ", Style::default().fg(COLOR_GHOST)),
            Span::styled("relay v0.1", Style::default().fg(COLOR_AMBER)),
        ]),
        Line::from(vec![
            Span::styled("  Data refresh: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{refresh_age} ago"),
                Style::default().fg(COLOR_GREEN),
            ),
            Span::styled(format!(" (every 900ms)"), Style::default().fg(COLOR_STEEL)),
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
            Span::styled("  ● Online: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", counts.online),
                Style::default().fg(COLOR_GREEN),
            ),
            Span::styled("  ◆ Working: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", counts.working),
                Style::default().fg(COLOR_AMBER),
            ),
            Span::styled("  ◐ Stale: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", counts.stale),
                Style::default().fg(COLOR_ORANGE),
            ),
            Span::styled("  ○ Offline: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", counts.offline),
                Style::default().fg(COLOR_RED),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Attention: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                stale_offline_watchlist(&state.agents),
                Style::default().fg(if counts.offline > 0 {
                    COLOR_RED
                } else {
                    COLOR_ORANGE
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Thresholds: ", Style::default().fg(COLOR_GHOST)),
            Span::styled("stale=", Style::default().fg(COLOR_STEEL)),
            Span::styled(
                format!("{AGENT_STALE_SECS}s"),
                Style::default().fg(COLOR_ORANGE),
            ),
            Span::styled("  offline=", Style::default().fg(COLOR_STEEL)),
            Span::styled(
                format!("{AGENT_OFFLINE_SECS}s"),
                Style::default().fg(COLOR_RED),
            ),
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
            "  relay is generic-first; Hermes metadata is additive when available.",
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
            let health = agent_health(&agent.status, agent.last_seen_epoch);
            let dot = match health {
                AgentHealth::Offline => "○",
                AgentHealth::Stale => "◐",
                AgentHealth::Working => "◆",
                AgentHealth::Online => "●",
            };
            let status_color = status_color(agent);
            let role = agent.role.as_deref().unwrap_or("-");
            let task = agent.task.as_deref().unwrap_or("");
            let marker = if selected { ">" } else { " " };
            let age_tag = human_age_short(agent.last_seen_epoch);
            let skills = state
                .hermes_snapshot
                .profile_skill_counts
                .get(&agent.name.to_lowercase())
                .copied()
                .unwrap_or(0);
            let skills_tag = if skills > 0 {
                format!(" | s:{skills}")
            } else {
                String::new()
            };

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
                    format!(
                        "{:<10} [{:<10}] {:<8}{}",
                        agent.name,
                        role,
                        health_label(health),
                        skills_tag
                    ),
                    style,
                ),
                Span::styled(
                    if task.is_empty() {
                        format!("  ({age_tag})")
                    } else {
                        format!(" :: {} ({age_tag})", truncate(task, 20))
                    },
                    Style::default().fg(COLOR_STEEL),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));

    let counts = health_counts(&state.agents);

    lines.push(Line::from(vec![
        Span::styled(" AGENTS: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", state.agents.len()),
            Style::default().fg(COLOR_GREEN),
        ),
        Span::styled(" | ONLINE: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", counts.online),
            Style::default().fg(COLOR_GREEN),
        ),
        Span::styled(" | WORKING: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", counts.working),
            Style::default().fg(COLOR_AMBER),
        ),
        Span::styled(" | STALE: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", counts.stale),
            Style::default().fg(COLOR_ORANGE),
        ),
        Span::styled(" | OFFLINE: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", counts.offline),
            Style::default().fg(COLOR_RED),
        ),
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
        let health = agent_health(&agent.status, agent.last_seen_epoch);
        let avatar = avatar::generate(&agent.name, None);
        lines.push(Line::from(Span::styled(
            format!("  {}", agent.name.to_uppercase()),
            Style::default()
                .fg(status_color(agent))
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  role: {}", agent.role.as_deref().unwrap_or("-")),
            Style::default().fg(COLOR_GHOST),
        )));
        lines.push(Line::from(Span::styled(
            format!("  health: {}", health_label(health)),
            Style::default().fg(status_color(agent)),
        )));
        let profile_skills = state
            .hermes_snapshot
            .profile_skill_counts
            .get(&agent.name.to_lowercase())
            .copied()
            .unwrap_or(0);
        lines.push(Line::from(Span::styled(
            format!("  hermes skills: {}", profile_skills),
            Style::default().fg(COLOR_CYAN),
        )));
        lines.push(Line::from(Span::styled(
            format!("  last seen: {} ago", human_age_long(agent.last_seen_epoch)),
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
                Style::default().fg(status_color(agent)),
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
        "[Enter] Send [[]/[ ]] Channels "
    } else {
        ""
    };

    let agent_hint = if state.active_tab == Tab::Agents {
        "[d] DM Selected "
    } else {
        ""
    };

    let status = Paragraph::new(Line::from(vec![
        Span::styled(" [Tab] Switch ", Style::default().fg(COLOR_STEEL)),
        Span::styled("[1-5] Tabs ", Style::default().fg(COLOR_STEEL)),
        Span::styled("[j/k] Select Agent ", Style::default().fg(COLOR_STEEL)),
        Span::styled(agent_hint, Style::default().fg(COLOR_STEEL)),
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

fn status_color(agent: &crate::storage::AgentPresence) -> Color {
    match agent_health(&agent.status, agent.last_seen_epoch) {
        AgentHealth::Offline => COLOR_RED,
        AgentHealth::Stale => COLOR_ORANGE,
        AgentHealth::Working => COLOR_AMBER,
        AgentHealth::Online => COLOR_GREEN,
    }
}

fn health_label(health: AgentHealth) -> &'static str {
    match health {
        AgentHealth::Offline => "offline",
        AgentHealth::Stale => "stale",
        AgentHealth::Working => "working",
        AgentHealth::Online => "online",
    }
}

fn agent_health(status: &str, last_seen_epoch: u64) -> AgentHealth {
    let age = now_unix_secs().saturating_sub(last_seen_epoch);
    if status == "offline" || age >= AGENT_OFFLINE_SECS {
        return AgentHealth::Offline;
    }
    if age >= AGENT_STALE_SECS {
        return AgentHealth::Stale;
    }
    if status == "working" {
        return AgentHealth::Working;
    }
    AgentHealth::Online
}

fn health_counts(agents: &[crate::storage::AgentPresence]) -> HealthCounts {
    let mut counts = HealthCounts::default();

    for agent in agents {
        match agent_health(&agent.status, agent.last_seen_epoch) {
            AgentHealth::Working => counts.working = counts.working.saturating_add(1),
            AgentHealth::Online => counts.online = counts.online.saturating_add(1),
            AgentHealth::Stale => counts.stale = counts.stale.saturating_add(1),
            AgentHealth::Offline => counts.offline = counts.offline.saturating_add(1),
        }
    }

    counts
}

fn stale_offline_watchlist(agents: &[crate::storage::AgentPresence]) -> String {
    let mut flagged = agents
        .iter()
        .filter_map(|agent| {
            let health = agent_health(&agent.status, agent.last_seen_epoch);
            match health {
                AgentHealth::Offline | AgentHealth::Stale => {
                    Some(format!("{}({})", agent.name, health_label(health)))
                }
                AgentHealth::Online | AgentHealth::Working => None,
            }
        })
        .collect::<Vec<String>>();

    if flagged.is_empty() {
        return "none".to_string();
    }

    flagged.sort();
    truncate(&flagged.join(", "), 64)
}

/// Health severity for sorting: 0=offline (worst), 1=stale, 2=online, 3=working.
fn health_severity(status: &str, last_seen_epoch: u64) -> u8 {
    match agent_health(status, last_seen_epoch) {
        AgentHealth::Offline => 0,
        AgentHealth::Stale => 1,
        AgentHealth::Online => 2,
        AgentHealth::Working => 3,
    }
}

/// Sort agents by health severity (worst first), then by name within each tier.
fn sort_agents_by_health(agents: &mut [crate::storage::AgentPresence]) {
    agents.sort_by(|a, b| {
        let sa = health_severity(&a.status, a.last_seen_epoch);
        let sb = health_severity(&b.status, b.last_seen_epoch);
        sa.cmp(&sb).then_with(|| a.name.cmp(&b.name))
    });
}

fn read_memory_files() -> Vec<(String, String)> {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return Vec::new();
    }
    let memories_dir = std::path::Path::new(&home).join(".hermes").join("memories");
    if !memories_dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let targets = ["USER.md", "MEMORY.md"];
    for name in targets {
        let path = memories_dir.join(name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                out.push((name.to_string(), trimmed));
            }
        }
    }
    out
}

fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1} KB");
    }
    let mb = kb / 1024.0;
    format!("{mb:.1} MB")
}

fn now_unix_secs() -> u64 {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn human_age_short(last_seen_epoch: u64) -> String {
    let age = now_unix_secs().saturating_sub(last_seen_epoch);
    if age < 60 {
        return format!("{age}s");
    }
    if age < 3600 {
        return format!("{}m", age / 60);
    }
    if age < 86_400 {
        return format!("{}h", age / 3600);
    }
    format!("{}d", age / 86_400)
}

fn human_age_long(last_seen_epoch: u64) -> String {
    let age = now_unix_secs().saturating_sub(last_seen_epoch);
    if age < 60 {
        return format!("{age}s");
    }
    let minutes = age / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {m}m", m = minutes % 60);
    }
    let days = hours / 24;
    format!("{days}d {h}h", h = hours % 24)
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
    let parsed = match epoch.parse::<u64>() {
        Ok(value) => value,
        Err(_) => 0,
    };
    let hhmmss = parsed % 86_400;
    let h = hhmmss / 3600;
    let m = (hhmmss % 3600) / 60;
    let s = hhmmss % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use crate::storage::AgentPresence;

    fn agent(name: &str, status: &str, last_seen_epoch: u64) -> AgentPresence {
        AgentPresence {
            name: name.to_string(),
            role: None,
            status: status.to_string(),
            task: None,
            last_seen_epoch,
        }
    }

    #[test]
    fn health_severity_offline_is_lowest() {
        assert_eq!(super::health_severity("offline", 0), 0);
        assert_eq!(super::health_severity("online", 0), 0); // age >= 300s => offline
    }

    #[test]
    fn health_severity_stale_above_offline() {
        // 100s ago, not offline status => stale
        let now = super::now_unix_secs();
        assert_eq!(super::health_severity("online", now - 100), 1);
    }

    #[test]
    fn health_severity_online_above_stale() {
        let now = super::now_unix_secs();
        assert_eq!(super::health_severity("online", now - 10), 2);
    }

    #[test]
    fn health_severity_working_highest() {
        let now = super::now_unix_secs();
        assert_eq!(super::health_severity("working", now - 10), 3);
    }

    #[test]
    fn sort_agents_by_health_puts_offline_first() {
        let now = super::now_unix_secs();
        let mut agents = vec![
            agent("alice", "working", now - 5),
            agent("charlie", "offline", now - 400),
            agent("bob", "online", now - 10),
        ];
        super::sort_agents_by_health(&mut agents);
        assert_eq!(agents[0].name, "charlie"); // offline first
        assert_eq!(agents[1].name, "bob"); // online second
        assert_eq!(agents[2].name, "alice"); // working last
    }

    #[test]
    fn sort_agents_by_health_breaks_ties_by_name() {
        let now = super::now_unix_secs();
        let mut agents = vec![
            agent("zebra", "online", now - 10),
            agent("alpha", "online", now - 10),
        ];
        super::sort_agents_by_health(&mut agents);
        assert_eq!(agents[0].name, "alpha");
        assert_eq!(agents[1].name, "zebra");
    }

    #[test]
    fn sort_agents_stale_before_online() {
        let now = super::now_unix_secs();
        let mut agents = vec![
            agent("online_agent", "online", now - 5),
            agent("stale_agent", "online", now - 100),
        ];
        super::sort_agents_by_health(&mut agents);
        assert_eq!(agents[0].name, "stale_agent");
        assert_eq!(agents[1].name, "online_agent");
    }

    #[test]
    fn watchlist_only_contains_stale_or_offline() {
        let now = super::now_unix_secs();
        let agents = vec![
            agent("online_agent", "online", now - 5),
            agent("working_agent", "working", now - 5),
            agent("stale_agent", "online", now - 100),
            agent("offline_agent", "offline", now - 400),
        ];

        let watchlist = super::stale_offline_watchlist(&agents);

        assert!(watchlist.contains("stale_agent(stale)"));
        assert!(watchlist.contains("offline_agent(offline)"));
        assert!(!watchlist.contains("online_agent"));
        assert!(!watchlist.contains("working_agent"));
    }
}
