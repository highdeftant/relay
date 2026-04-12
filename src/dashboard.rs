//! TUI dashboard — Ratatui + crossterm event loop.

use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    DefaultTerminal, Frame,
};

use crate::{app::{AppState, Tab}, config::AppConfig};

/// Entry point for `relay watch`.
pub async fn watch(_config: AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnterAlternateScreen)?;

    let result = run_dashboard(&mut terminal).await;

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    ratatui::restore();

    result
}

async fn run_dashboard(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut state = AppState::new();

    loop {
        terminal.draw(|frame| draw(frame, &state))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                handle_key(&mut state, key.code);
            }
        }

        if state.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_key(state: &mut AppState, code: KeyCode) {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => state.should_quit = true,
        KeyCode::Tab => state.active_tab = state.active_tab.next(),
        KeyCode::BackTab => state.active_tab = state.active_tab.prev(),
        KeyCode::Char('1') => state.active_tab = Tab::Chat,
        KeyCode::Char('2') => state.active_tab = Tab::Agents,
        KeyCode::Char('3') => state.active_tab = Tab::Files,
        KeyCode::Char('4') => state.active_tab = Tab::Logs,
        _ => {}
    }
}

fn draw(frame: &mut Frame<'_>, state: &AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title bar
            Constraint::Length(3), // tab bar
            Constraint::Min(1),   // content
            Constraint::Length(1), // status bar
        ])
        .split(frame.area());

    draw_title(frame, outer[0]);
    draw_tabs(frame, outer[1], state.active_tab);
    draw_content(frame, outer[2], state);
    draw_status(frame, outer[3], state);
}

/// Title bar: 同期//SYNC — agent count, uptime
fn draw_title(frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " 同期//SYNC ",
            Style::default()
                .fg(Color::Rgb(0, 255, 65)) // terminal green
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("RELAY v0.1", Style::default().fg(Color::Rgb(232, 232, 240))), // ghost white
        Span::styled("  ──  ", Style::default().fg(Color::Rgb(42, 42, 53))), // rust steel
        Span::styled("AGENTS: ", Style::default().fg(Color::Rgb(232, 232, 240))),
        Span::styled(
            format!("{:02}", state_agent_count_placeholder()),
            Style::default().fg(Color::Rgb(0, 255, 65)),
        ),
        Span::styled("  |  STATUS: ", Style::default().fg(Color::Rgb(232, 232, 240))),
        Span::styled("ONLINE", Style::default().fg(Color::Rgb(0, 255, 65))),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(42, 42, 53))),
    );

    frame.render_widget(title, area);
}

/// Tab bar with active indicator
fn draw_tabs(frame: &mut Frame<'_>, area: ratatui::layout::Rect, active: Tab) {
    let titles: Vec<Line<'_>> = Tab::ALL
        .iter()
        .map(|t| {
            let is_active = *t == active;
            let prefix = if is_active { "> " } else { "  " };
            let style = if is_active {
                Style::default()
                    .fg(Color::Rgb(0, 255, 65)) // terminal green
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(232, 232, 240)) // ghost white dimmed
            };
            Line::from(vec![Span::styled(format!("{prefix}{}", t.label()), style)])
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::Rgb(0, 229, 255))), // phantom cyan
        )
        .style(Style::default().bg(Color::Rgb(10, 10, 15))) // void black
        .divider(Span::styled(
            "  │  ",
            Style::default().fg(Color::Rgb(42, 42, 53)),
        ));

    frame.render_widget(tabs, area);
}

/// Main content area — switches based on active tab
fn draw_content(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    match state.active_tab {
        Tab::Chat => draw_chat_placeholder(frame, area),
        Tab::Agents => draw_agents_placeholder(frame, area),
        Tab::Files => draw_files_placeholder(frame, area),
        Tab::Logs => draw_logs_placeholder(frame, area),
    }
}

fn draw_chat_placeholder(frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    let content = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Channel: #general",
            Style::default().fg(Color::Rgb(0, 229, 255)), // phantom cyan
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [channel messages will appear here]",
            Style::default().fg(Color::Rgb(42, 42, 53)), // rust steel
        )),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  > _",
            Style::default().fg(Color::Rgb(232, 232, 240)),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(0, 229, 255)))
            .title(Span::styled(
                " #general ",
                Style::default().fg(Color::Rgb(0, 229, 255)),
            )),
    );

    frame.render_widget(content, area);
}

fn draw_agents_placeholder(frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    let content = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  [agent list + avatars will render here]",
            Style::default().fg(Color::Rgb(42, 42, 53)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press Enter on an agent to view profile",
            Style::default().fg(Color::Rgb(42, 42, 53)),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(0, 229, 255)))
            .title(Span::styled(
                " 接続//AGENTS ",
                Style::default().fg(Color::Rgb(0, 229, 255)),
            )),
    );

    frame.render_widget(content, area);
}

fn draw_files_placeholder(frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    let content = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  [file transfers will appear here]",
            Style::default().fg(Color::Rgb(42, 42, 53)),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(0, 229, 255)))
            .title(Span::styled(
                " ファイル//FILES ",
                Style::default().fg(Color::Rgb(0, 229, 255)),
            )),
    );

    frame.render_widget(content, area);
}

fn draw_logs_placeholder(frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    let content = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  [server events and agent activity will appear here]",
            Style::default().fg(Color::Rgb(42, 42, 53)),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(0, 229, 255)))
            .title(Span::styled(
                " ログ//LOGS ",
                Style::default().fg(Color::Rgb(0, 229, 255)),
            )),
    );

    frame.render_widget(content, area);
}

/// Status bar at the bottom
fn draw_status(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let status = Paragraph::new(Line::from(vec![
        Span::styled(
            " [Tab] Switch  [1-4] Tab  [q] Quit ",
            Style::default().fg(Color::Rgb(42, 42, 53)),
        ),
        Span::styled("│", Style::default().fg(Color::Rgb(42, 42, 53))),
        Span::styled(
            format!(" Tab: {} ", state.active_tab.label()),
            Style::default().fg(Color::Rgb(255, 176, 0)), // data amber
        ),
    ]));

    frame.render_widget(status, area);
}

/// Placeholder until we wire up real agent count.
fn state_agent_count_placeholder() -> usize {
    0
}
