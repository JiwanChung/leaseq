use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Line, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{App, Focus, Mode, NodeModalAction};

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    } else {
        s.to_string()
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    if app.logs_state.maximized {
        // Maximized logs view: header + logs + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(0),     // Logs (takes all space)
                Constraint::Length(1),  // Footer
            ])
            .split(f.area());

        draw_header(f, app, chunks[0]);
        draw_logs(f, app, chunks[1]);
        draw_footer(f, app, chunks[2]);
    } else {
        // Normal view: header + top row (nodes|tasks|detail) + logs + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(8),     // Top row (Nodes | Tasks | Detail)
                Constraint::Length(10), // Logs pane
                Constraint::Length(1),  // Footer
            ])
            .split(f.area());

        // Split top row into 3 columns
        let top_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(20),  // Nodes
                Constraint::Percentage(45),  // Tasks
                Constraint::Percentage(35),  // Task Detail
            ])
            .split(chunks[1]);

        draw_header(f, app, chunks[0]);
        draw_nodes(f, app, top_row[0]);
        draw_tasks(f, app, top_row[1]);
        draw_task_detail(f, app, top_row[2]);
        draw_logs(f, app, chunks[2]);
        draw_footer(f, app, chunks[3]);
    }

    if app.mode == Mode::InputAdd {
        draw_add_task_popup(f, app);
    }

    if app.mode == Mode::CreateLease {
        draw_create_lease_popup(f, app);
    }

    if app.mode == Mode::NodeDetails {
        draw_node_details_popup(f, app);
    }

    if app.mode == Mode::Help {
        draw_help_popup(f);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let title = Paragraph::new(format!(" LeaseQ Monitor | Lease: {} ", app.lease_id))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, area);
}

fn draw_nodes(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focus == Focus::Nodes;
    let border_style = if is_focused { Style::default().fg(Color::Yellow) } else { Style::default() };
    
    let items: Vec<ListItem> = app
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let status_color = if n.status == "OK" { Color::Green } else { Color::Red };
            let content = Line::from(vec![
                Span::styled(format!("{:<15}", n.name), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!(" [{}]", n.status), Style::default().fg(status_color)),
                Span::raw(format!(" {:.0}s", n.last_seen)),
            ]);
            
            if i == app.selected_node_idx && is_focused {
                ListItem::new(content).style(Style::default().bg(Color::DarkGray))
            } else {
                ListItem::new(content)
            }
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Nodes ").border_style(border_style));
    f.render_widget(list, area);
}

fn draw_tasks(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focus == Focus::Tasks;
    let border_style = if is_focused { Style::default().fg(Color::Yellow) } else { Style::default() };

    // Show filter in title
    let filter_str = format!("{}", app.filter_state.filter);
    let title = format!(" Tasks [{}] ", filter_str);

    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let state_color = match t.state.as_str() {
                "RUNNING" => Color::Green,
                "PENDING" => Color::Yellow,
                "DONE" => Color::Blue,
                "FAILED" => Color::Red,
                _ => Color::White,
            };

            let exit_info = if let Some(code) = t.exit_code {
                if code != 0 { format!(" [{}]", code) } else { String::new() }
            } else {
                String::new()
            };

            // Show short ID (first 8 chars) for readability
            let short_id: String = t.id.chars().take(8).collect();

            // GPU indicator
            let gpu_indicator = if t.gpus_requested > 0 {
                format!("G{}", t.gpus_requested)
            } else {
                "  ".to_string()
            };

            // Truncate command if too long (keep it readable)
            let cmd_display = if t.command.len() > 30 {
                format!("{}...", &t.command[..27])
            } else {
                t.command.clone()
            };

            let content = Line::from(vec![
                Span::styled(format!("{:<8}", short_id), Style::default().fg(state_color).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {:<7}", t.state), Style::default().fg(state_color)),
                Span::styled(format!(" {:>2}", gpu_indicator), Style::default().fg(Color::Magenta)),
                Span::styled(format!(" {:<10}", truncate_str(&t.node, 10)), Style::default().fg(Color::Gray)),
                Span::raw(format!(" {}{}", cmd_display, exit_info)),
            ]);

            if i == app.selected_task_idx && is_focused {
                 ListItem::new(content).style(Style::default().bg(Color::DarkGray))
            } else {
                 ListItem::new(content)
            }
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title).border_style(border_style));
    f.render_widget(list, area);
}

fn draw_task_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Detail ")
        .style(Style::default().fg(Color::Gray));

    if let Some(task) = app.selected_task() {
        let state_color = match task.state.as_str() {
            "RUNNING" => Color::Green,
            "PENDING" => Color::Yellow,
            "DONE" => Color::Blue,
            "FAILED" => Color::Red,
            _ => Color::White,
        };

        let exit_str = task.exit_code.map(|c| format!("{}", c)).unwrap_or_else(|| "-".to_string());

        // GPU display
        let gpu_str = if task.gpus_requested == 0 {
            "CPU".to_string()
        } else if task.gpus_assigned.is_empty() {
            format!("{} (pending)", task.gpus_requested)
        } else {
            format!("{} [{}]", task.gpus_requested, task.gpus_assigned)
        };

        // Vertical layout for column display
        let lines = vec![
            Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&task.id, Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("State: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&task.state, Style::default().fg(state_color).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("Node: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&task.node),
            ]),
            Line::from(vec![
                Span::styled("GPUs: ", Style::default().fg(Color::DarkGray)),
                Span::styled(gpu_str, Style::default().fg(Color::Magenta)),
            ]),
            Line::from(vec![
                Span::styled("Exit: ", Style::default().fg(Color::DarkGray)),
                Span::raw(exit_str),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Command:", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled(&task.command, Style::default().fg(Color::Cyan)),
            ]),
        ];

        let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
        f.render_widget(p, area);
    } else {
        let p = Paragraph::new("(No task selected)")
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        f.render_widget(p, area);
    }
}

fn draw_logs(f: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focus == Focus::Logs;
    let border_style = if is_focused { Style::default().fg(Color::Yellow) } else { Style::default() };

    let task_label = app.logs_state.task_id.as_deref().unwrap_or("(none)");
    let stream = if app.logs_state.show_stderr { "stderr" } else { "stdout" };
    let follow_indicator = if app.logs_state.auto_follow { " [FOLLOW]" } else { "" };
    let max_indicator = if app.logs_state.maximized { " [MAX]" } else { "" };
    let title = format!(" Logs: {} ({}){}{}  ", task_label, stream, follow_indicator, max_indicator);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style);

    // Calculate visible lines based on area height
    let inner_height = area.height.saturating_sub(2) as usize; // account for borders
    app.log_view_height = inner_height; // Store for Ctrl+U/D scrolling
    let total_lines = app.logs_state.lines.len();

    let start = if app.logs_state.auto_follow {
        total_lines.saturating_sub(inner_height)
    } else {
        app.logs_state.scroll.min(total_lines.saturating_sub(inner_height))
    };

    let visible_lines: Vec<Line> = app
        .logs_state
        .lines
        .iter()
        .skip(start)
        .take(inner_height)
        .map(|s| Line::from(s.as_str()))
        .collect();

    let p = Paragraph::new(visible_lines)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(p, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    // Show status message if present, otherwise show help
    if let Some((msg, _)) = &app.status_message {
        let p = Paragraph::new(msg.as_str())
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        f.render_widget(p, area);
    } else {
        let text = if app.logs_state.maximized {
            if app.logs_state.auto_follow {
                "Enter/z:Minimize | f:Static | e:Stderr | g:Top | Backspace:Tasks | q:Quit | ?:Help"
            } else {
                "Enter/z:Minimize | f:Follow | e:Stderr | j/k:Scroll | ^u/d:Page | g/G:Jump | q:Quit"
            }
        } else {
            "h/j/k/l:Nav | Enter:Select | z:Zoom | F:Filter | a:Add | n:Lease | e:Stderr | q:Quit | ?:Help"
        };
        let p = Paragraph::new(text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(p, area);
    }
}

fn draw_add_task_popup(f: &mut Frame, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Add Task ")
        .style(Style::default().fg(Color::Cyan));
    let area = centered_rect(60, 20, f.area());
    f.render_widget(Clear, area); // Clear background
    #[allow(deprecated)]
    f.render_widget(app.textarea.widget(), block.inner(area));
    f.render_widget(block, area);
}

fn draw_create_lease_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 55, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Create Slurm Lease (Tab to cycle, Enter to Submit) ")
        .style(Style::default().fg(Color::Magenta));

    f.render_widget(block.clone(), area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3), // Partition
            Constraint::Length(3), // GPUs
            Constraint::Length(3), // QoS
            Constraint::Length(3), // Nodes
            Constraint::Length(3), // Time
            Constraint::Length(3), // Wait
        ])
        .split(area);

    let inputs = [
        ("Partition", &app.lease_form.partition),
        ("GPUs/Node", &app.lease_form.gpus),
        ("QoS (empty=default)", &app.lease_form.qos),
        ("Nodes (default: 1)", &app.lease_form.nodes),
        ("Time (empty=unlimited)", &app.lease_form.time),
        ("Wait secs (0=no wait)", &app.lease_form.wait),
    ];

    for (i, (label, textarea)) in inputs.iter().enumerate() {
        let is_active = i == app.lease_form.active_field;
        let style = if is_active { Style::default().fg(Color::Yellow) } else { Style::default() };
        let block = Block::default().borders(Borders::ALL).title(*label).style(style);

        #[allow(deprecated)]
        f.render_widget(textarea.widget(), block.inner(chunks[i]));
        f.render_widget(block, chunks[i]);
    }
}

fn draw_node_details_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 35, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Node Actions (j/k to select, Enter to confirm) ")
        .style(Style::default().fg(Color::Cyan));

    f.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Node info
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Option 1
            Constraint::Length(1), // Option 2
        ])
        .split(inner);

    // Node info
    if let Some(node) = app.nodes.get(app.selected_node_idx) {
        let status_color = if node.status == "OK" { Color::Green } else { Color::Red };
        let info = Line::from(vec![
            Span::raw("Node: "),
            Span::styled(&node.name, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  Status: "),
            Span::styled(&node.status, Style::default().fg(status_color)),
            Span::raw(format!("  Last seen: {:.0}s ago", node.last_seen)),
        ]);
        f.render_widget(Paragraph::new(info), chunks[0]);
    }

    // Options
    let options = [
        ("View Status", NodeModalAction::ViewStatus),
        ("Release Lease", NodeModalAction::ReleaseLease),
    ];

    for (i, (label, action)) in options.iter().enumerate() {
        let is_selected = app.node_modal.selected == *action;
        let style = if is_selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let prefix = if is_selected { "> " } else { "  " };
        let text = format!("{}{}", prefix, label);
        f.render_widget(
            Paragraph::new(text).style(style),
            chunks[2 + i],
        );
    }
}

fn draw_help_popup(f: &mut Frame) {
    let area = centered_rect(60, 80, f.area());
    let block = Block::default().borders(Borders::ALL).title(" Help ").style(Style::default().bg(Color::Blue));
    let text = vec![
        "Pane Navigation:",
        "  h/l      Move left/right between panes",
        "  j/k      Navigate lists (or scroll logs when zoomed)",
        "  Tab      Cycle: Nodes -> Tasks -> Logs -> Nodes",
        "  Backspace  Return to Tasks from Logs",
        "",
        "Actions:",
        "  Enter    Nodes: open details",
        "           Tasks: view logs & focus Logs pane",
        "           Logs: toggle zoom (maximize/minimize)",
        "  a        Add Task (opens input)",
        "  n        New Slurm Lease (opens form)",
        "  F        Cycle task filter (Recent/All/Running/...)",
        "",
        "Task Filters:",
        "  Recent   All active + recent completed (default)",
        "  All      Show all tasks",
        "  Running  Only running tasks",
        "  Pending  Only pending tasks",
        "  Done     Only successful tasks",
        "  Failed   Only failed tasks",
        "",
        "Logs Behavior:",
        "  Normal view: always follows (auto-scroll)",
        "  Zoomed view: toggle follow with 'f'",
        "",
        "Logs Navigation (zoomed + static mode only):",
        "  j/k      Scroll 1 line",
        "  Ctrl+u/d Scroll half page",
        "  g        Jump to start",
        "  G        Jump to end (enables follow)",
        "  f        Toggle follow/static mode",
        "",
        "Other:",
        "  z        Toggle zoom logs",
        "  e        Toggle stdout/stderr",
        "  q        Quit",
        "  ?        Show this help",
        "  Esc      Close popups",
    ];
    let p = Paragraph::new(Text::from(text.join("\n")))
        .block(block)
        .alignment(Alignment::Left);

    f.render_widget(Clear, area);
    f.render_widget(p, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
