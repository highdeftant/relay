//! TUI dashboard — Ratatui + crossterm event loop.

use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
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
    gateway_health, hermes,
    profiles::load_hermes_admission_allowlist,
    protocol, storage,
    types::UnixEpochSecs,
};

// Dashboard palette, brightened slightly for terminal contrast.
const COLOR_VOID: Color = Color::Rgb(30, 36, 34); // deep neutral background
const COLOR_STEEL: Color = Color::Rgb(50, 64, 54); // surface/border shadow green
const COLOR_GHOST: Color = Color::Rgb(230, 214, 186); // text/hints
const COLOR_GREEN: Color = Color::Rgb(92, 122, 96); // online/accent
const COLOR_CYAN: Color = Color::Rgb(230, 214, 186); // bright accent
const COLOR_AMBER: Color = Color::Rgb(230, 214, 186); // warning-alt
const COLOR_ORANGE: Color = Color::Rgb(198, 72, 58); // stale/active
const COLOR_RED: Color = Color::Rgb(226, 94, 78); // danger

const AGENT_STALE_SECS: u64 = 90;
const AGENT_OFFLINE_SECS: u64 = 300;
const AGENT_WARNING_SECS: u64 = 60; // begin warning at 60s (30s before stale)
const MAX_LOG_LINES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentHealth {
    Working,
    Online,
    Warning, // approaching stale threshold
    Stale,
    Offline,
}

#[derive(Debug, Default, Clone, Copy)]
struct HealthCounts {
    working: usize,
    online: usize,
    warning: usize,
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
            && let Some(action) = action_for_key_event(&mut state, key)
        {
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
                                push_log(&mut state, format!("send_message failed: {error}"));
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

        if state.should_quit {
            break;
        }
    }

    Ok(())
}

fn refresh_state(config: &AppConfig, state: &mut AppState) {
    state.hermes_snapshot = hermes::load_snapshot();
    state.gateway_health =
        gateway_health::check_all_endpoints(&gateway_health::default_profile_endpoints());

    state.last_refresh_unix = now_unix_secs();
    let allowed_profiles = load_hermes_admission_allowlist();
    if !allowed_profiles.contains(&state.chat_agent.to_lowercase()) {
        state.chat_agent = allowed_profiles
            .iter()
            .cloned()
            .min()
            .unwrap_or_else(|| "hermes".to_string());
    }

    match storage::load_agents(config) {
        Ok(map) => {
            let mut rows = map.into_values().collect::<Vec<_>>();
            rows.retain(|agent| allowed_profiles.contains(&agent.name.to_lowercase()));
            sort_agents_by_health(&mut rows);
            state.agents = rows;
            state.clamp_selection();
        }
        Err(error) => {
            push_log(state, format!("load_agents failed: {error}"));
        }
    }

    match storage::list_channels(config) {
        Ok(channels) => state.set_channels(channels),
        Err(error) => push_log(state, format!("list_channels failed: {error}")),
    }

    match storage::load_channel_events(config, &state.active_channel, 200) {
        Ok(events) => state.messages = events,
        Err(error) => push_log(
            state,
            format!(
                "load_channel_events failed for {}: {error}",
                state.active_channel
            ),
        ),
    }

    trim_logs(&mut state.logs);
}

fn push_log(state: &mut AppState, message: String) {
    state.logs.push(message);
    trim_logs(&mut state.logs);
}

fn trim_logs(logs: &mut Vec<String>) {
    if logs.len() > MAX_LOG_LINES {
        let keep_from = logs.len().saturating_sub(MAX_LOG_LINES);
        logs.drain(0..keep_from);
    }
}

fn action_for_key_event(state: &mut AppState, key: KeyEvent) -> Option<UiAction> {
    if key.kind == KeyEventKind::Release {
        return None;
    }

    handle_key(state, key.code, key.modifiers)
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
        KeyCode::Char('1') if state.active_tab != Tab::Chat => {
            state.active_tab = Tab::Chat;
            return None;
        }
        KeyCode::Char('2') if state.active_tab != Tab::Chat => {
            state.active_tab = Tab::Agents;
            return None;
        }
        KeyCode::Char('3') if state.active_tab != Tab::Chat => {
            state.active_tab = Tab::Knowledge;
            return None;
        }
        KeyCode::Char('4') if state.active_tab != Tab::Chat => {
            state.active_tab = Tab::Memory;
            return None;
        }
        KeyCode::Char('5') if state.active_tab != Tab::Chat => {
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
            KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r') => {
                return Some(UiAction::SendChatMessage);
            }
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

    // Keep fleet counts visible even when no one is currently online.
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
    if counts.warning > 0 {
        spans.push(Span::styled("  ", Style::default().fg(COLOR_STEEL)));
        spans.push(Span::styled("◌", Style::default().fg(COLOR_ORANGE)));
        spans.push(Span::styled(
            format!("{}", counts.warning),
            Style::default().fg(COLOR_AMBER),
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

    let mut lines: Vec<Line<'_>> = vec![
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
            Span::styled("  ◌ Warn: ", Style::default().fg(COLOR_GHOST)),
            Span::styled(
                format!("{}", counts.warning),
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
            Span::styled("warn=", Style::default().fg(COLOR_STEEL)),
            Span::styled(
                format!("{AGENT_WARNING_SECS}s"),
                Style::default().fg(COLOR_AMBER),
            ),
            Span::styled("  stale=", Style::default().fg(COLOR_STEEL)),
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
        Line::from(Span::styled(
            format!("  {}", health_bar(&counts)),
            Style::default().fg(COLOR_GHOST),
        )),
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
    ];

    // ── Gateway connectivity section ──
    let gateways_online = state.gateway_health.iter().filter(|h| h.reachable).count();
    let gateways_total = state.gateway_health.len();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  網関//GATEWAY CONNECTIVITY",
            Style::default().fg(COLOR_CYAN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {gateways_online}/{gateways_total} up"),
            Style::default().fg(if gateways_online == gateways_total {
                COLOR_GREEN
            } else {
                COLOR_ORANGE
            }),
        ),
    ]));
    for h in &state.gateway_health {
        let color = if h.reachable { COLOR_GREEN } else { COLOR_RED };
        let icon = if h.reachable { "●" } else { "○" };
        let latency = h
            .latency_ms
            .map(|ms| format!("{ms}ms"))
            .unwrap_or_else(|| "unreachable".into());
        lines.push(Line::from(vec![
            Span::styled(format!("  {icon} "), Style::default().fg(color)),
            Span::styled(
                format!("{:<10}", h.profile),
                Style::default().fg(COLOR_GHOST),
            ),
            Span::styled(
                format!(" @ {:<18}", h.endpoint),
                Style::default().fg(COLOR_STEEL),
            ),
            Span::styled(latency, Style::default().fg(color)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Hermes-exclusive mode: only locally defined Hermes profiles can connect.",
        Style::default().fg(COLOR_STEEL),
    )));

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
                AgentHealth::Warning => "◌",
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
        Span::styled(" | WARN: ", Style::default().fg(COLOR_GHOST)),
        Span::styled(
            format!("{:02}", counts.warning),
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
        if let Some(hint) = health_transition_hint(&agent.status, agent.last_seen_epoch) {
            let hint_color = match health {
                AgentHealth::Warning => COLOR_AMBER,
                AgentHealth::Stale => COLOR_ORANGE,
                _ => COLOR_STEEL,
            };
            lines.push(Line::from(Span::styled(
                format!("  next: {hint}"),
                Style::default().fg(hint_color),
            )));
        }
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
        Span::styled(" [Tab] Switch ", Style::default().fg(COLOR_GHOST)),
        Span::styled("[1-5] Tabs ", Style::default().fg(COLOR_GHOST)),
        Span::styled("[j/k] Select Agent ", Style::default().fg(COLOR_GHOST)),
        Span::styled(agent_hint, Style::default().fg(COLOR_GHOST)),
        Span::styled(chat_hint, Style::default().fg(COLOR_GHOST)),
        Span::styled("[q] Quit ", Style::default().fg(COLOR_GHOST)),
        Span::styled("│", Style::default().fg(COLOR_GREEN)),
        Span::styled(
            format!(" {} ", state.active_tab.label()),
            Style::default()
                .fg(COLOR_GREEN)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    frame.render_widget(status, area);
}

fn status_color(agent: &crate::storage::AgentPresence) -> Color {
    match agent_health(&agent.status, agent.last_seen_epoch) {
        AgentHealth::Offline => COLOR_RED,
        AgentHealth::Stale => COLOR_ORANGE,
        AgentHealth::Warning => COLOR_ORANGE,
        AgentHealth::Working => COLOR_AMBER,
        AgentHealth::Online => COLOR_GREEN,
    }
}

fn health_label(health: AgentHealth) -> &'static str {
    match health {
        AgentHealth::Offline => "offline",
        AgentHealth::Stale => "stale",
        AgentHealth::Warning => "warning",
        AgentHealth::Working => "working",
        AgentHealth::Online => "online",
    }
}

fn agent_health(status: &str, last_seen_epoch: UnixEpochSecs) -> AgentHealth {
    let age = now_unix_secs().saturating_sub(last_seen_epoch);
    if status == "offline" || age >= AGENT_OFFLINE_SECS {
        return AgentHealth::Offline;
    }
    if age >= AGENT_STALE_SECS {
        return AgentHealth::Stale;
    }
    if age >= AGENT_WARNING_SECS {
        return AgentHealth::Warning;
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
            AgentHealth::Warning => counts.warning = counts.warning.saturating_add(1),
            AgentHealth::Stale => counts.stale = counts.stale.saturating_add(1),
            AgentHealth::Offline => counts.offline = counts.offline.saturating_add(1),
        }
    }

    counts
}

fn health_bar(counts: &HealthCounts) -> String {
    let total = counts.working + counts.online + counts.warning + counts.stale + counts.offline;
    if total == 0 {
        return "[no agents]".to_string();
    }
    let bar_width = 40usize;
    let w = (counts.working * bar_width) / total;
    let o = (counts.online * bar_width) / total;
    let wn = (counts.warning * bar_width) / total;
    let s = (counts.stale * bar_width) / total;
    let of = bar_width.saturating_sub(w + o + wn + s);

    let mut bar = String::from("[");
    for _ in 0..w {
        bar.push('◆');
    }
    for _ in 0..o {
        bar.push('●');
    }
    for _ in 0..wn {
        bar.push('◌');
    }
    for _ in 0..s {
        bar.push('◐');
    }
    for _ in 0..of {
        bar.push('○');
    }
    bar.push(']');

    format!(
        "{bar} {}/{}",
        counts.working + counts.online + counts.warning,
        total
    )
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
                AgentHealth::Warning => {
                    let age = now_unix_secs().saturating_sub(agent.last_seen_epoch);
                    let until_stale = AGENT_STALE_SECS.saturating_sub(age);
                    Some(format!("{}(stale in {}s)", agent.name, until_stale))
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

/// Health severity for sorting: 0=offline (worst), 1=stale, 2=warning, 3=online, 4=working.
fn health_severity(status: &str, last_seen_epoch: UnixEpochSecs) -> u8 {
    match agent_health(status, last_seen_epoch) {
        AgentHealth::Offline => 0,
        AgentHealth::Stale => 1,
        AgentHealth::Warning => 2,
        AgentHealth::Online => 3,
        AgentHealth::Working => 4,
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

fn now_unix_secs() -> UnixEpochSecs {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn human_age_short(last_seen_epoch: UnixEpochSecs) -> String {
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

fn human_age_long(last_seen_epoch: UnixEpochSecs) -> String {
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

fn health_transition_hint(status: &str, last_seen_epoch: UnixEpochSecs) -> Option<String> {
    let age = now_unix_secs().saturating_sub(last_seen_epoch);
    let health = agent_health(status, last_seen_epoch);
    match health {
        AgentHealth::Online | AgentHealth::Working => {
            let until_warning = AGENT_WARNING_SECS.saturating_sub(age);
            if until_warning > 0 {
                Some(format!(
                    "warning in {}",
                    human_age_short_from_secs(until_warning)
                ))
            } else {
                None
            }
        }
        AgentHealth::Warning => {
            let until_stale = AGENT_STALE_SECS.saturating_sub(age);
            if until_stale > 0 {
                Some(format!(
                    "stale in {}",
                    human_age_short_from_secs(until_stale)
                ))
            } else {
                None
            }
        }
        AgentHealth::Stale => {
            let until_offline = AGENT_OFFLINE_SECS.saturating_sub(age);
            if until_offline > 0 {
                Some(format!(
                    "offline in {}",
                    human_age_short_from_secs(until_offline)
                ))
            } else {
                None
            }
        }
        AgentHealth::Offline => None,
    }
}

fn human_age_short_from_secs(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    format!("{}h", secs / 3600)
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
    use crate::{
        storage::AgentPresence,
        types::{AgentName, AgentStatus, UnixEpochSecs},
    };

    fn agent(name: &str, status: &str, last_seen_epoch: UnixEpochSecs) -> AgentPresence {
        AgentPresence {
            name: AgentName::from(name),
            role: None,
            status: AgentStatus::from(status),
            task: None,
            last_seen_epoch,
        }
    }

    #[test]
    fn chat_mode_allows_typing_numeric_characters() {
        let mut state = crate::app::AppState::new();
        state.active_tab = crate::app::Tab::Chat;

        let action = super::handle_key(
            &mut state,
            crossterm::event::KeyCode::Char('1'),
            crossterm::event::KeyModifiers::NONE,
        );

        assert!(action.is_none());
        assert_eq!(state.active_tab, crate::app::Tab::Chat);
        assert_eq!(state.chat_input, "1");
    }

    #[test]
    fn numeric_shortcuts_switch_tabs_outside_chat() {
        let mut state = crate::app::AppState::new();
        state.active_tab = crate::app::Tab::Agents;

        let action = super::handle_key(
            &mut state,
            crossterm::event::KeyCode::Char('5'),
            crossterm::event::KeyModifiers::NONE,
        );

        assert!(action.is_none());
        assert_eq!(state.active_tab, crate::app::Tab::System);
    }

    #[test]
    fn chat_mode_enter_triggers_send_action() {
        let mut state = crate::app::AppState::new();
        state.active_tab = crate::app::Tab::Chat;
        state.chat_input = "hello".to_string();

        let action = super::handle_key(
            &mut state,
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );

        assert!(matches!(action, Some(super::UiAction::SendChatMessage)));
    }

    #[test]
    fn chat_mode_newline_char_triggers_send_action() {
        let mut state = crate::app::AppState::new();
        state.active_tab = crate::app::Tab::Chat;
        state.chat_input = "hello".to_string();

        let action = super::handle_key(
            &mut state,
            crossterm::event::KeyCode::Char('\n'),
            crossterm::event::KeyModifiers::NONE,
        );

        assert!(matches!(action, Some(super::UiAction::SendChatMessage)));
    }

    #[test]
    fn chat_mode_carriage_return_char_triggers_send_action() {
        let mut state = crate::app::AppState::new();
        state.active_tab = crate::app::Tab::Chat;
        state.chat_input = "hello".to_string();

        let action = super::handle_key(
            &mut state,
            crossterm::event::KeyCode::Char('\r'),
            crossterm::event::KeyModifiers::NONE,
        );

        assert!(matches!(action, Some(super::UiAction::SendChatMessage)));
    }

    #[test]
    fn key_release_is_ignored_for_chat_send() {
        let mut state = crate::app::AppState::new();
        state.active_tab = crate::app::Tab::Chat;
        state.chat_input = "hello".to_string();

        let release_action = super::action_for_key_event(
            &mut state,
            crossterm::event::KeyEvent::new_with_kind(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
                crossterm::event::KeyEventKind::Release,
            ),
        );
        let press_action = super::action_for_key_event(
            &mut state,
            crossterm::event::KeyEvent::new_with_kind(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
                crossterm::event::KeyEventKind::Press,
            ),
        );

        assert!(release_action.is_none());
        assert!(matches!(
            press_action,
            Some(super::UiAction::SendChatMessage)
        ));
    }

    #[test]
    fn health_severity_offline_is_lowest() {
        assert_eq!(super::health_severity("offline", 0), 0);
        assert_eq!(super::health_severity("online", 0), 0);
    }

    #[test]
    fn health_severity_stale_above_offline() {
        let now = super::now_unix_secs();
        assert_eq!(super::health_severity("online", now - 100), 1);
    }

    #[test]
    fn health_severity_warning_between_stale_and_online() {
        let now = super::now_unix_secs();
        assert_eq!(super::health_severity("online", now - 70), 2);
    }

    #[test]
    fn health_severity_online_above_warning() {
        let now = super::now_unix_secs();
        assert_eq!(super::health_severity("online", now - 10), 3);
    }

    #[test]
    fn health_severity_working_highest() {
        let now = super::now_unix_secs();
        assert_eq!(super::health_severity("working", now - 10), 4);
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
        assert_eq!(agents[0].name.as_ref(), "charlie");
        assert_eq!(agents[1].name.as_ref(), "bob");
        assert_eq!(agents[2].name.as_ref(), "alice");
    }

    #[test]
    fn sort_agents_by_health_breaks_ties_by_name() {
        let now = super::now_unix_secs();
        let mut agents = vec![
            agent("zebra", "online", now - 10),
            agent("alpha", "online", now - 10),
        ];
        super::sort_agents_by_health(&mut agents);
        assert_eq!(agents[0].name.as_ref(), "alpha");
        assert_eq!(agents[1].name.as_ref(), "zebra");
    }

    #[test]
    fn sort_agents_stale_before_online() {
        let now = super::now_unix_secs();
        let mut agents = vec![
            agent("online_agent", "online", now - 5),
            agent("stale_agent", "online", now - 100),
        ];
        super::sort_agents_by_health(&mut agents);
        assert_eq!(agents[0].name.as_ref(), "stale_agent");
        assert_eq!(agents[1].name.as_ref(), "online_agent");
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

    #[test]
    fn profile_allowlist_parses_profile_names() {
        let root =
            std::env::temp_dir().join(format!("relay-dashboard-allowlist-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let profiles_path = root.join("profiles.json");
        let payload = r#"[
            {"name":"Hermes","role":"coordinator","created":"2026-01-01","bio":"","skills":[],"color":"cyan","avatar":"default","avatar_file":null},
            {"name":" Claude ","role":"reviewer","created":"2026-01-01","bio":"","skills":[],"color":"green","avatar":"default","avatar_file":null}
        ]"#;
        assert!(std::fs::write(&profiles_path, payload).is_ok());

        let allowlist = crate::profiles::load_profile_allowlist(&profiles_path);
        assert!(allowlist.contains("hermes"));
        assert!(allowlist.contains("claude"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn watchlist_includes_warning_agents_with_countdown() {
        let now = super::now_unix_secs();
        let agents = vec![
            agent("online_agent", "online", now - 5),
            agent("warning_agent", "online", now - 70),
        ];

        let watchlist = super::stale_offline_watchlist(&agents);

        assert!(watchlist.contains("warning_agent(stale in"));
        assert!(!watchlist.contains("online_agent"));
    }

    #[test]
    fn sort_agents_warning_between_stale_and_online() {
        let now = super::now_unix_secs();
        let mut agents = vec![
            agent("online_agent", "online", now - 5),
            agent("warning_agent", "online", now - 70),
            agent("stale_agent", "online", now - 100),
        ];
        super::sort_agents_by_health(&mut agents);
        assert_eq!(agents[0].name.as_ref(), "stale_agent");
        assert_eq!(agents[1].name.as_ref(), "warning_agent");
        assert_eq!(agents[2].name.as_ref(), "online_agent");
    }

    #[test]
    fn health_bar_empty_for_no_agents() {
        let counts = super::HealthCounts::default();
        assert_eq!(super::health_bar(&counts), "[no agents]");
    }

    #[test]
    fn health_bar_shows_all_online() {
        let counts = super::HealthCounts {
            online: 4,
            ..Default::default()
        };
        let bar = super::health_bar(&counts);
        assert!(bar.contains("4/4"));
        assert!(bar.contains("●"));
    }

    #[test]
    fn health_transition_hint_online_shows_warning_countdown() {
        let now = super::now_unix_secs();
        let hint = super::health_transition_hint("online", now - 10);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("warning in"));
    }

    #[test]
    fn health_transition_hint_warning_shows_stale_countdown() {
        let now = super::now_unix_secs();
        let hint = super::health_transition_hint("online", now - 70);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("stale in"));
    }

    #[test]
    fn health_transition_hint_stale_shows_offline_countdown() {
        let now = super::now_unix_secs();
        let hint = super::health_transition_hint("online", now - 100);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("offline in"));
    }

    #[test]
    fn health_transition_hint_offline_returns_none() {
        let hint = super::health_transition_hint("offline", 0);
        assert!(hint.is_none());
    }

    #[test]
    fn health_counts_includes_warning() {
        let now = super::now_unix_secs();
        let agents = vec![
            agent("a", "online", now - 5),
            agent("b", "online", now - 70),
            agent("c", "online", now - 100),
        ];
        let counts = super::health_counts(&agents);
        assert_eq!(counts.online, 1);
        assert_eq!(counts.warning, 1);
        assert_eq!(counts.stale, 1);
    }
}
