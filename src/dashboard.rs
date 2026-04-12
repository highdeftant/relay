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
    hermes,
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
            state.active_tab = Tab::Skills;
            return None;
        }
        KeyCode::Char('4') => {
            state.active_tab = Tab::Sessions;
            return None;
        }
        KeyCode::Char('5') => {
            state.active_tab = Tab::Memory;
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
            KeyCode::Char('d') => state.open_dm_with_selected(),
            _ => {}
        },
        Tab::Chat => match code {
            KeyCode::Enter => return Some(UiAction::SendChatMessage),
            KeyCode::Char(']') if state.chat_input.is_empty() => return Some(UiAction::NextChannel),
            KeyCode::Char('[') if state.chat_input.is_empty() => return Some(UiAction::PrevChannel),
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
        Tab::Skills | Tab::Sessions | Tab::Memory | Tab::System => {}
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
        Tab::Skills => draw_skills_panel(frame, area, state),
        Tab::Sessions => draw_sessions_panel(frame, area, state),
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
                format!(" #{}  [{} / {}] ", state.active_channel, state.channels.len(), channels_line),
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

fn draw_skills_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines = vec![Line::from("")];
    let snapshot = &state.hermes_snapshot;

    lines.push(Line::from(vec![
        Span::styled("  Hermes skills detected: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(format!("{}", snapshot.skill_count), Style::default().fg(COLOR_GREEN)),
    ]));

    lines.push(Line::from(vec![
        Span::styled("  Skills root: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            if snapshot.skills_root_exists { "present" } else { "missing" },
            Style::default().fg(if snapshot.skills_root_exists {
                COLOR_GREEN
            } else {
                COLOR_RED
            }),
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
        for category in snapshot.skill_categories.iter().take(16) {
            lines.push(Line::from(vec![
                Span::styled("   - ", Style::default().fg(COLOR_STEEL)),
                Span::styled(category.clone(), Style::default().fg(COLOR_GHOST)),
            ]));
        }
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(" HERMES//SKILLS ", Style::default().fg(COLOR_CYAN))),
    );

    frame.render_widget(widget, area);
}

fn draw_sessions_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines = vec![Line::from("")];
    let snapshot = &state.hermes_snapshot;

    lines.push(Line::from(vec![
        Span::styled("  Session files: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(format!("{}", snapshot.session_count), Style::default().fg(COLOR_GREEN)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  recent",
        Style::default().fg(COLOR_CYAN).add_modifier(Modifier::BOLD),
    )));

    if snapshot.recent_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none found",
            Style::default().fg(COLOR_STEEL),
        )));
    } else {
        for session in &snapshot.recent_sessions {
            lines.push(Line::from(vec![
                Span::styled("   - ", Style::default().fg(COLOR_STEEL)),
                Span::styled(truncate(session, 72), Style::default().fg(COLOR_GHOST)),
            ]));
        }
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(" HERMES//SESSIONS ", Style::default().fg(COLOR_CYAN))),
    );

    frame.render_widget(widget, area);
}

fn draw_memory_panel(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let snapshot = &state.hermes_snapshot;

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  state.db: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                if snapshot.state_db_exists {
                    format!("present ({} bytes)", snapshot.state_db_bytes)
                } else {
                    "missing".to_string()
                },
                Style::default().fg(if snapshot.state_db_exists {
                    COLOR_GREEN
                } else {
                    COLOR_RED
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("  honcho hosts: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(format!("{}", snapshot.honcho_hosts), Style::default().fg(COLOR_CYAN)),
        ]),
        Line::from(vec![
            Span::styled("  auth.json: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                if snapshot.auth_exists { "present" } else { "missing" },
                Style::default().fg(if snapshot.auth_exists { COLOR_GREEN } else { COLOR_RED }),
            ),
        ]),
        Line::from(vec![
            Span::styled("  processes.json: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                if snapshot.processes_file_exists {
                    format!("present ({} entries)", snapshot.known_process_count)
                } else {
                    "missing".to_string()
                },
                Style::default().fg(if snapshot.processes_file_exists {
                    COLOR_GREEN
                } else {
                    COLOR_RED
                }),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Note: relay supports all agents; Hermes adds local skill/session/memory visibility.",
            Style::default().fg(COLOR_STEEL),
        )),
    ];

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_CYAN))
            .title(Span::styled(" HERMES//MEMORY ", Style::default().fg(COLOR_CYAN))),
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
                "CHAT AGENTS SKILLS SESSIONS MEMORY SYSTEM",
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
            let dot = if agent.status == "offline" {
                "○"
            } else {
                "●"
            };
            let status_color = status_color(&agent.status);
            let role = agent.role.as_deref().unwrap_or("-");
            let task = agent.task.as_deref().unwrap_or("");
            let marker = if selected { ">" } else { " " };
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
                    format!("{:<10} [{:<10}] {:<8}{}", agent.name, role, agent.status, skills_tag),
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
        Span::styled("[1-6] Tabs ", Style::default().fg(COLOR_STEEL)),
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
