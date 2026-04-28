use std::{
    collections::BTreeSet,
    io,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::*,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, Wrap},
    Terminal,
};

use crate::{
    actions::{
        estimate_reclaim_bytes, has_supported_steps, spawn_operation, ActionEvent, ActionReport,
        OperationKind,
    },
    config::Config,
    i18n::{detect_system_language, Language},
    model::{human_size, Asset, RemovalAdvice, SourceKind},
    plugins::{spawn_scanner, ScanEvent},
};

pub fn run(config: Config) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, config);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, config: Config) -> Result<()> {
    let language = detect_system_language();
    let mut app = App::new(config.clone(), language, spawn_scanner(config, language));

    loop {
        app.drain_scan_events();
        app.drain_action_events();
        terminal.draw(|frame| app.render(frame))?;

        if event::poll(Duration::from_millis(120))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if app.handle_key(key.code) {
                break;
            }
        }

        app.tick();
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Loading,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingIntent {
    Action(OperationKind),
    Refresh,
    Quit,
}

struct App {
    config: Config,
    language: Language,
    screen: Screen,
    scan_rx: Receiver<ScanEvent>,
    scan_completed: usize,
    scan_total: usize,
    scan_found: usize,
    scan_label: String,
    spinner_index: usize,
    assets: Vec<Asset>,
    cursor: usize,
    selected: BTreeSet<String>,
    pending_intent: Option<PendingIntent>,
    running_action: Option<RunningAction>,
    action_rx: Option<Receiver<ActionEvent>>,
    awaiting_password: bool,
    password_input: String,
    password_error: Option<String>,
    jump_input: String,
    jump_updated_at: Option<Instant>,
    search_mode: bool,
    search_query: String,
    message: String,
    post_scan_message: Option<String>,
    action_feedback: Option<ActionFeedback>,
}

#[derive(Debug, Clone)]
struct RunningAction {
    operation: OperationKind,
    label: String,
    completed: usize,
    total: usize,
}

#[derive(Debug, Clone)]
struct ActionFeedback {
    report: ActionReport,
    shown_at: Instant,
}

impl App {
    fn new(config: Config, language: Language, scan_rx: Receiver<ScanEvent>) -> Self {
        let scan_total = enabled_source_count(&config);
        Self {
            config,
            language,
            screen: Screen::Loading,
            scan_rx,
            scan_completed: 0,
            scan_total,
            scan_found: 0,
            scan_label: String::new(),
            spinner_index: 0,
            assets: Vec::new(),
            cursor: 0,
            selected: BTreeSet::new(),
            pending_intent: None,
            running_action: None,
            action_rx: None,
            awaiting_password: false,
            password_input: String::new(),
            password_error: None,
            jump_input: String::new(),
            jump_updated_at: None,
            search_mode: false,
            search_query: String::new(),
            message: if language.is_zh() {
                "加载中".to_string()
            } else {
                "Loading".to_string()
            },
            post_scan_message: None,
            action_feedback: None,
        }
    }

    fn tick(&mut self) {
        self.spinner_index = (self.spinner_index + 1) % SPINNER.len();
        if self
            .jump_updated_at
            .is_some_and(|time| time.elapsed() > Duration::from_millis(900))
        {
            self.clear_jump_input();
        }
        if self
            .action_feedback
            .as_ref()
            .is_some_and(|feedback| feedback.shown_at.elapsed() > Duration::from_secs(2))
        {
            self.action_feedback = None;
        }
    }

    fn drain_scan_events(&mut self) {
        while let Ok(event) = self.scan_rx.try_recv() {
            match event {
                ScanEvent::Progress {
                    current,
                    completed,
                    total,
                    found,
                } => {
                    self.scan_completed = completed;
                    self.scan_total = total;
                    self.scan_found = found;
                    self.scan_label = if self.language.is_zh() {
                        format!("正在扫描 {}...", current.label())
                    } else {
                        format!("Scanning {}...", current.label())
                    };
                }
                ScanEvent::Finished(assets) => {
                    self.assets = assets;
                    self.screen = Screen::Ready;
                    self.clamp_cursor();
                    self.selected.retain(|id| self.assets.iter().any(|asset| &asset.id == id));
                    self.message = self.post_scan_message.take().unwrap_or_else(|| {
                        if self.assets.is_empty() {
                            self.tr("未找到任何资产。", "No assets found.")
                        } else {
                            self.tr(
                                "扫描完成。空格多选，Delete 删除，C 清缓存。",
                                "Scan complete. Space to select, Delete to remove, C to clean cache.",
                            )
                        }
                    });
                }
            }
        }
    }

    fn drain_action_events(&mut self) {
        let mut finished = None;
        let mut admin_prompt = None;

        if let Some(rx) = &self.action_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ActionEvent::Progress {
                        operation,
                        label,
                        completed,
                        total,
                    } => {
                        self.running_action = Some(RunningAction {
                            operation,
                            label,
                            completed,
                            total,
                        });
                    }
                    ActionEvent::AdminPrompt { operation, retry } => {
                        admin_prompt = Some((operation, retry));
                    }
                    ActionEvent::Finished(report) => {
                        finished = Some(report);
                    }
                }
            }
        }

        if let Some((operation, retry)) = admin_prompt {
            self.action_rx = None;
            self.running_action = None;
            self.pending_intent = Some(PendingIntent::Action(operation));
            self.awaiting_password = true;
            self.password_input.clear();
            self.password_error = Some(if retry {
                self.tr(
                    "密码不正确，请重新输入管理员密码后回车。",
                    "Incorrect password. Re-enter the admin password and press Enter.",
                )
            } else {
                self.tr(
                    "此操作需要管理员密码。请输入后按回车执行。",
                    "This action needs an admin password. Enter it and press Enter to continue.",
                )
            });
        }

        if let Some(report) = finished {
            self.finish_action(report);
        }
    }

    fn handle_key(&mut self, code: KeyCode) -> bool {
        if self.running_action.is_some() {
            if matches!(code, KeyCode::Esc) {
                self.message = self.tr(
                    "正在执行系统操作，请等待完成后再退出。",
                    "System operation is running. Please wait for it to finish.",
                );
            }
            return false;
        }

        if self.search_mode {
            return self.handle_search_input(code);
        }

        if let Some(intent) = self.pending_intent {
            return self.handle_pending_intent(code, intent);
        }

        match self.screen {
            Screen::Loading => {
                if matches!(code, KeyCode::Esc) {
                    return true;
                }
            }
            Screen::Ready => match code {
                KeyCode::Esc => self.prepare_quit(),
                KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
                KeyCode::Char(' ') => self.toggle_selection(),
                KeyCode::Backspace | KeyCode::Delete => self.prepare_action(OperationKind::Delete),
                KeyCode::Char('c') | KeyCode::Char('C') => self.prepare_action(OperationKind::CleanCache),
                KeyCode::Char('r') | KeyCode::Char('R') => self.prepare_refresh(),
                KeyCode::Char('/') => self.enter_search_mode(),
                KeyCode::Char('s') | KeyCode::Char('S') => self.sort_by_size_desc(),
                KeyCode::Char(ch) if ch.is_ascii_digit() => self.handle_jump_digit(ch),
                _ => {}
            },
        }

        false
    }

    fn handle_pending_intent(&mut self, code: KeyCode, intent: PendingIntent) -> bool {
        if self.awaiting_password {
            return self.handle_password_input(code, intent);
        }

        match intent {
            PendingIntent::Action(action) => match code {
                KeyCode::Enter => self.begin_action(action),
                KeyCode::Backspace | KeyCode::Delete if action == OperationKind::Delete => {
                    self.begin_action(action)
                }
                KeyCode::Char('c') | KeyCode::Char('C') if action == OperationKind::CleanCache => {
                    self.begin_action(action)
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.clear_pending_intent();
                    self.message = self.tr("已取消操作。", "Action canceled.");
                }
                _ => {}
            },
            PendingIntent::Refresh => match code {
                KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => self.manual_refresh(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.clear_pending_intent();
                    self.message = self.tr("已取消刷新。", "Refresh canceled.");
                }
                _ => {}
            },
            PendingIntent::Quit => match code {
                KeyCode::Enter | KeyCode::Esc => return true,
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.clear_pending_intent();
                    self.message = self.tr("已取消退出。", "Quit canceled.");
                }
                _ => {}
            },
        }

        false
    }

    fn handle_password_input(&mut self, code: KeyCode, intent: PendingIntent) -> bool {
        match code {
            KeyCode::Esc => {
                self.clear_pending_intent();
                self.message = self.tr("已取消操作。", "Action canceled.");
            }
            KeyCode::Enter => {
                if self.password_input.is_empty() {
                    self.password_error = Some(self.tr(
                        "请输入管理员密码后再回车。",
                        "Enter the admin password before pressing Enter.",
                    ));
                    return false;
                }

                if let PendingIntent::Action(action) = intent {
                    self.begin_action_with_password(action, self.password_input.clone());
                }
            }
            KeyCode::Backspace => {
                self.password_input.pop();
            }
            KeyCode::Char(ch) => {
                self.password_input.push(ch);
            }
            _ => {}
        }

        false
    }

    fn handle_search_input(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Enter => {
                self.search_mode = false;
                self.message = if self.search_query.is_empty() {
                    self.tr("已退出搜索。", "Search closed.")
                } else {
                    self.tr(
                        &format!("搜索中：{}", self.search_query),
                        &format!("Filtering: {}", self.search_query),
                    )
                };
            }
            KeyCode::Esc => {
                self.search_mode = false;
                self.search_query.clear();
                self.clamp_cursor();
                self.message = self.tr("已清空搜索。", "Search cleared.");
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.clamp_cursor();
            }
            KeyCode::Char(ch) => {
                self.search_query.push(ch);
                self.clamp_cursor();
            }
            _ => {}
        }

        false
    }

    fn begin_action(&mut self, operation: OperationKind) {
        let assets = self.selected_assets();
        if assets.is_empty() {
            self.clear_pending_intent();
            self.message = self.tr("没有可执行的选中项。", "No actionable items selected.");
            return;
        }

        self.awaiting_password = false;
        self.password_input.clear();
        self.password_error = None;
        self.action_feedback = None;
        self.action_rx = Some(spawn_operation(operation, assets, None));
        self.running_action = Some(RunningAction {
            operation,
            label: if self.language.is_zh() {
                format!("{}准备开始", operation.progress_label())
            } else {
                format!("Preparing {}", operation.progress_label())
            },
            completed: 0,
            total: self.selection_count(),
        });
        self.clear_pending_intent();
        self.message = if self.language.is_zh() {
            format!("{}已启动，请稍候。", operation.label())
        } else {
            format!("{} started. Please wait.", operation.label())
        };
    }

    fn begin_action_with_password(&mut self, operation: OperationKind, password: String) {
        let assets = self.selected_assets();
        if assets.is_empty() {
            self.clear_pending_intent();
            self.message = self.tr("没有可执行的选中项。", "No actionable items selected.");
            return;
        }

        self.awaiting_password = false;
        self.password_error = None;
        self.action_feedback = None;
        self.action_rx = Some(spawn_operation(operation, assets, Some(password)));
        self.running_action = Some(RunningAction {
            operation,
            label: if self.language.is_zh() {
                format!("{}准备开始", operation.progress_label())
            } else {
                format!("Preparing {}", operation.progress_label())
            },
            completed: 0,
            total: self.selection_count(),
        });
        self.message = if self.language.is_zh() {
            format!("{}已启动，请稍候。", operation.label())
        } else {
            format!("{} started. Please wait.", operation.label())
        };
    }

    fn finish_action(&mut self, report: ActionReport) {
        self.action_rx = None;
        self.running_action = None;
        self.apply_action_report(&report);
        self.clear_pending_intent();
        self.message = action_message(&report, self.language);
        self.action_feedback = Some(ActionFeedback {
            report,
            shown_at: Instant::now(),
        });
    }

    fn restart_scan(&mut self) {
        self.screen = Screen::Loading;
        self.scan_completed = 0;
        self.scan_total = enabled_source_count(&self.config);
        self.scan_found = 0;
        self.action_feedback = None;
        self.clear_pending_intent();
        self.clear_jump_input();
        self.scan_label = if self.language.is_zh() {
            "正在重新扫描资产...".to_string()
        } else {
            "Rescanning assets...".to_string()
        };
        self.scan_rx = spawn_scanner(self.config.clone(), self.language);
    }

    fn apply_action_report(&mut self, report: &ActionReport) {
        match report.operation {
            OperationKind::Delete => {
                let removed_ids = report
                    .outputs
                    .iter()
                    .filter(|output| output.success)
                    .filter_map(|output| output.asset_id.clone())
                    .collect::<BTreeSet<_>>();

                if !removed_ids.is_empty() {
                    self.assets.retain(|asset| !removed_ids.contains(&asset.id));
                }
                self.selected.retain(|id| !removed_ids.contains(id));
                self.clamp_cursor();
            }
            OperationKind::CleanCache => {}
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }

        let len = len as isize;
        let next = (self.cursor as isize + delta).clamp(0, len - 1);
        self.cursor = next as usize;
    }

    fn toggle_selection(&mut self) {
        if let Some(asset_id) = self.current_asset().map(|asset| asset.id.clone()) {
            if !self.selected.insert(asset_id.clone()) {
                self.selected.remove(&asset_id);
            }
        }
    }

    fn prepare_action(&mut self, action: OperationKind) {
        self.clear_jump_input();
        if self.selection_count() == 0 {
            self.message = match action {
                OperationKind::Delete => self.tr(
                    "请先选中至少一个项目，再按 Delete 删除。",
                    "Select at least one item before pressing Delete.",
                ),
                OperationKind::CleanCache => self.tr(
                    "请先选中至少一个项目，再按 C 清缓存。",
                    "Select at least one item before pressing C to clean cache.",
                ),
            };
            return;
        }
        let selected_assets = self.selected_assets();
        if !has_supported_steps(action, &selected_assets) {
            self.message = match action {
                OperationKind::Delete => self.tr("当前选中项暂不支持删除。", "Delete is not supported for the current selection."),
                OperationKind::CleanCache => self.tr("当前选中项暂不支持清理缓存。", "Cache cleanup is not supported for the current selection."),
            };
            return;
        }
        self.pending_intent = Some(PendingIntent::Action(action));
        self.message = match action {
            OperationKind::Delete => self.tr(
                "删除操作已就绪，请在右侧操作区确认。",
                "Delete is ready. Confirm it in the action panel.",
            ),
            OperationKind::CleanCache => self.tr(
                "清缓存操作已就绪，请在右侧操作区确认。",
                "Cache cleanup is ready. Confirm it in the action panel.",
            ),
        };
    }

    fn clear_pending_intent(&mut self) {
        self.pending_intent = None;
        self.awaiting_password = false;
        self.password_input.clear();
        self.password_error = None;
    }

    fn manual_refresh(&mut self) {
        self.clear_jump_input();
        self.clear_pending_intent();
        self.message = self.tr("正在刷新列表...", "Refreshing the list...");
        self.restart_scan();
    }

    fn prepare_refresh(&mut self) {
        self.clear_jump_input();
        self.pending_intent = Some(PendingIntent::Refresh);
        self.message = self.tr(
            "刷新操作已就绪，请在右侧操作区确认。",
            "Refresh is ready. Confirm it in the action panel.",
        );
    }

    fn prepare_quit(&mut self) {
        self.clear_jump_input();
        self.pending_intent = Some(PendingIntent::Quit);
        self.message = self.tr(
            "退出操作已就绪，请在右侧操作区确认。",
            "Quit is ready. Confirm it in the action panel.",
        );
    }

    fn enter_search_mode(&mut self) {
        self.clear_pending_intent();
        self.clear_jump_input();
        self.search_mode = true;
        self.message = self.tr(
            "搜索模式：输入名称实时过滤，Enter 保留结果，Esc 清空并退出。",
            "Search mode: type to filter live, Enter keeps the result, Esc clears and exits.",
        );
    }

    fn handle_jump_digit(&mut self, digit: char) {
        let visible_len = self.visible_len();
        if visible_len == 0 {
            return;
        }

        let stale = self
            .jump_updated_at
            .map_or(true, |time| time.elapsed() > Duration::from_millis(900));
        if stale {
            self.jump_input.clear();
        }

        self.jump_input.push(digit);
        self.jump_updated_at = Some(Instant::now());

        let Ok(number) = self.jump_input.parse::<usize>() else {
            self.clear_jump_input();
            return;
        };

        if number == 0 {
            return;
        }

        if number <= visible_len {
            self.cursor = number - 1;
            self.message = if self.language.is_zh() {
                format!("已跳转到第 {number} 项。")
            } else {
                format!("Jumped to item {number}.")
            };
        } else {
            self.message = if self.language.is_zh() {
                format!("没有第 {number} 项。")
            } else {
                format!("Item {number} does not exist.")
            };
        }
    }

    fn clear_jump_input(&mut self) {
        self.jump_input.clear();
        self.jump_updated_at = None;
    }

    fn sort_by_size_desc(&mut self) {
        self.assets.sort_by(|left, right| right.size_bytes.cmp(&left.size_bytes));
        self.clamp_cursor();
        self.message = self.tr(
            "已按空间占用从大到小排序。",
            "Sorted by size from largest to smallest.",
        );
    }

    fn current_asset(&self) -> Option<&Asset> {
        let index = *self.visible_indices().get(self.cursor)?;
        self.assets.get(index)
    }

    fn selection_count(&self) -> usize {
        self.selected.len()
    }

    fn selected_assets(&self) -> Vec<Asset> {
        self.assets
            .iter()
            .filter(|asset| self.selected.contains(&asset.id))
            .cloned()
            .collect()
    }

    fn reclaimable_bytes(&self) -> u64 {
        self.assets
            .iter()
            .filter(|asset| self.selected.contains(&asset.id))
            .map(|asset| asset.size_bytes)
            .sum()
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.search_query.trim().to_ascii_lowercase();
        self.assets
            .iter()
            .enumerate()
            .filter(|(_, asset)| {
                query.is_empty()
                    || asset.name.to_ascii_lowercase().contains(&query)
                    || asset.source.label().to_ascii_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn visible_len(&self) -> usize {
        self.visible_indices().len()
    }

    fn clamp_cursor(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(len - 1);
        }
    }

    fn render(&self, frame: &mut Frame) {
        match self.screen {
            Screen::Loading => self.render_loading(frame),
            Screen::Ready => self.render_ready(frame),
        }
    }

    fn render_loading(&self, frame: &mut Frame) {
        frame.render_widget(
            Block::default().style(Style::default().bg(color_bg()).fg(color_text())),
            frame.area(),
        );

        let area = centered_rect(78, 16, frame.area());
        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" PKGOH ", brand_style()),
                Span::raw(" "),
                Span::styled(self.tr("加载中", "Loading"), section_title_style()),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color_cyan()))
            .style(panel_style())
            .border_type(BorderType::Plain);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);

        let inner = inner(area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);

        let logo = Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled("PKGOH", Style::default().fg(color_cyan()).add_modifier(Modifier::BOLD))),
        ]))
        .alignment(Alignment::Center);

        let ratio = if self.scan_total == 0 {
            0.0
        } else {
            self.scan_completed as f64 / self.scan_total as f64
        };

        let title = if self.scan_label.is_empty() {
            self.tr("加载中", "Loading")
        } else {
            self.scan_label.clone()
        };
        let progress_line = if self.language.is_zh() {
            format!(
                "{} {}    进度 {}/{}    已发现 {}",
                SPINNER[self.spinner_index],
                title,
                self.scan_completed,
                self.scan_total,
                self.scan_found
            )
        } else {
            format!(
                "{} {}    {} / {}    Found {}",
                SPINNER[self.spinner_index],
                title,
                self.scan_completed,
                self.scan_total,
                self.scan_found
            )
        };

        frame.render_widget(logo, rows[0]);
        frame.render_widget(
            Paragraph::new(progress_line)
                .alignment(Alignment::Center)
                .style(Style::default().fg(color_text())),
            rows[1],
        );
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .label(format!("{:.0}%", ratio * 100.0))
                .gauge_style(Style::default().fg(color_cyan()).bg(color_panel())),
            rows[2],
        );
        frame.render_widget(
            Paragraph::new(self.tr(
                "正在整理你的终端资产，请稍候…",
                "Preparing your terminal asset inventory…",
            ))
            .alignment(Alignment::Center)
            .style(Style::default().fg(color_dim()))
            .wrap(Wrap { trim: true }),
            rows[4],
        );
    }

    fn render_ready(&self, frame: &mut Frame) {
        frame.render_widget(
            Block::default().style(Style::default().bg(color_bg()).fg(color_text())),
            frame.area(),
        );

        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(14),
                Constraint::Length(9),
                Constraint::Length(4),
            ])
            .split(frame.area());

        self.render_header(frame, areas[0]);

        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(areas[1]);
        self.render_table(frame, top[0]);
        self.render_detail(frame, top[1]);
        self.render_action_panel(frame, areas[2]);
        self.render_footer(frame, areas[3]);

        if let Some(intent) = self.pending_intent {
            self.render_overlay(frame, intent);
        } else if self.running_action.is_some() {
            self.render_running_overlay(frame);
        } else if let Some(feedback) = &self.action_feedback {
            self.render_result_overlay(frame, feedback);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let block = panel_block(" ", Style::default().fg(color_cyan()));
        frame.render_widget(block, area);

        let inner = inner(area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(rows[0]);
        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(rows[1]);

        let status_text = self.current_status_text();
        let status_style = self.current_status_style();
        let left_line = Line::from(vec![
            Span::styled(" PKGOH ", brand_style()),
            Span::raw("  "),
            Span::styled(status_text, status_style),
        ]);

        let total_label = if self.search_query.is_empty() {
            self.assets.len().to_string()
        } else {
            format!("{}/{}", self.visible_len(), self.assets.len())
        };
        let stats = if self.language.is_zh() {
            format!(
                "总数 {}  已选 {}  可释放 {}  排序 大小↓",
                total_label,
                self.selection_count(),
                human_size(self.reclaimable_bytes()),
            )
        } else {
            format!(
                "Total {}  Selected {}  Reclaim {}  Sort Size↓",
                total_label,
                self.selection_count(),
                human_size(self.reclaimable_bytes()),
            )
        };
        let modules = self.tr(
            "来源 All  模块 Homebrew · npm · pnpm · cargo · pip · uv · mas",
            "Source All  Modules Homebrew · npm · pnpm · cargo · pip · uv · mas",
        );
        let search = if self.search_query.is_empty() {
            self.tr("搜索 全部", "Search all")
        } else if self.search_mode {
            self.tr(
                &format!("搜索 {}  · 输入中", self.search_query),
                &format!("Search {}  · typing", self.search_query),
            )
        } else {
            self.tr(
                &format!("搜索 {}", self.search_query),
                &format!("Search {}", self.search_query),
            )
        };
        let aux = if self.jump_input.is_empty() {
            search
        } else if self.language.is_zh() {
            format!("{search}  · 跳转 {0}", self.jump_input)
        } else {
            format!("{search}  · Jump {}", self.jump_input)
        };

        frame.render_widget(
            Paragraph::new(left_line).style(Style::default().fg(color_text())),
            top[0],
        );
        frame.render_widget(
            Paragraph::new(stats)
                .alignment(Alignment::Right)
                .style(Style::default().fg(color_text())),
            top[1],
        );
        frame.render_widget(
            Paragraph::new(modules).style(Style::default().fg(color_dim())),
            bottom[0],
        );
        frame.render_widget(
            Paragraph::new(aux)
                .alignment(Alignment::Right)
                .style(Style::default().fg(color_dim())),
            bottom[1],
        );
    }

    fn render_table(&self, frame: &mut Frame, area: Rect) {
        let visible_indices = self.visible_indices();
        if visible_indices.is_empty() {
            let body = if self.search_query.is_empty() {
                self.tr("未找到任何资产。", "No assets found.")
            } else {
                self.tr("没有匹配当前搜索词的项目。", "No assets match the current search.")
            };
            frame.render_widget(
                Paragraph::new(body)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(color_dim()))
                    .block(panel_block(
                        &format!(" {} ", self.tr("资产列表", "Assets")),
                        Style::default().fg(color_cyan()),
                    )),
                area,
            );
            return;
        }

        let header = Row::new([
            Cell::from("#"),
            Cell::from(self.tr("选", "Sel")),
            Cell::from(self.tr("名称", "Name")),
            Cell::from(self.tr("来源", "Source")),
            Cell::from(self.tr("版本", "Version")),
            Cell::from(self.tr("大小", "Size")),
            Cell::from(self.tr("最近使用", "Last Used")),
        ])
        .style(Style::default().fg(color_cyan()).add_modifier(Modifier::BOLD));

        let max_rows = area.height.saturating_sub(4).max(1) as usize;
        let (start, end) = visible_window(self.cursor, visible_indices.len(), max_rows);

        let rows: Vec<Row> = visible_indices[start..end]
            .iter()
            .enumerate()
            .map(|(offset, asset_index)| {
                let visible_index = start + offset;
                let asset = &self.assets[*asset_index];
                let is_selected = self.selected.contains(&asset.id);
                let is_focused = visible_index == self.cursor;

                let marker = if is_selected { "●" } else { "○" };
                let name = highlight_query(
                    &asset.name,
                    self.search_query.trim(),
                    is_focused,
                    color_cyan(),
                );

                let mut row_style = Style::default().fg(color_text()).bg(color_panel());
                if is_selected {
                    row_style = row_style.add_modifier(Modifier::BOLD);
                }
                if is_focused {
                    row_style = Style::default()
                        .fg(color_bg())
                        .bg(color_cyan())
                        .add_modifier(Modifier::BOLD);
                }

                let size_style = if is_focused {
                    row_style
                } else if asset.is_large(&self.config.highlight) {
                    Style::default().fg(color_warn()).bg(color_panel()).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color_text()).bg(color_panel())
                };
                let used_style = if is_focused {
                    row_style
                } else if asset.is_stale(&self.config.highlight) {
                    Style::default().fg(color_danger()).bg(color_panel())
                } else {
                    Style::default().fg(color_dim()).bg(color_panel())
                };

                Row::new([
                    Cell::from(format!("{:>2}.", visible_index + 1)),
                    Cell::from(marker).style(if is_selected {
                        Style::default()
                            .fg(if is_focused { color_bg() } else { color_cyan() })
                            .bg(if is_focused { color_cyan() } else { color_panel() })
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(if is_focused { color_bg() } else { color_dim() })
                            .bg(if is_focused { color_cyan() } else { color_panel() })
                    }),
                    Cell::from(name),
                    Cell::from(asset.source.label()).style(if is_focused {
                        row_style
                    } else {
                        Style::default().fg(color_dim()).bg(color_panel())
                    }),
                    Cell::from(asset.version.clone()).style(if is_focused {
                        row_style
                    } else {
                        Style::default().fg(color_dim()).bg(color_panel())
                    }),
                    Cell::from(asset.size_label()).style(size_style),
                    Cell::from(asset.last_used_label()).style(used_style),
                ])
                .style(row_style)
            })
            .collect();

        let title = if self.search_query.is_empty() {
            format!(" {} · {} ", self.tr("资产列表", "Assets"), visible_indices.len())
        } else {
            format!(
                " {} · {}/{} ",
                self.tr("资产列表", "Assets"),
                visible_indices.len(),
                self.assets.len()
            )
        };

        let table = Table::new(
            rows,
            [
                Constraint::Length(4),
                Constraint::Length(5),
                Constraint::Percentage(33),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(12),
            ],
        )
        .header(header)
        .block(panel_block(&title, Style::default().fg(color_cyan())))
        .column_spacing(1)
        .style(Style::default().bg(color_panel()).fg(color_text()));

        frame.render_widget(table, area);
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let detail_title = format!(" {} ", self.tr("详情", "Details"));
        let block = panel_block(&detail_title, Style::default().fg(color_cyan()));
        frame.render_widget(block, area);

        let Some(asset) = self.current_asset() else {
            frame.render_widget(
                Paragraph::new(self.tr("未选中项目", "No asset selected"))
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(color_dim()))
                    .block(panel_block(
                        &format!(" {} ", self.tr("详情", "Details")),
                        Style::default().fg(color_cyan()),
                    )),
                area,
            );
            return;
        };

        let inner = inner(area);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(6),
                Constraint::Length(6),
                Constraint::Min(5),
            ])
            .split(inner);

        let title_spans = vec![
            Span::styled(asset.name.clone(), Style::default().fg(color_text()).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(asset.source.label().to_string(), Style::default().fg(color_dim())),
        ];

        let tags = detail_tags(asset, &self.config, self.language);
        let title_body = Text::from(vec![
            Line::from(title_spans),
            Line::from(Span::styled(
                format!("{} {}", self.tr("版本", "Version"), asset.version),
                Style::default().fg(color_dim()),
            )),
            Line::from(tags),
        ]);
        frame.render_widget(
            Paragraph::new(title_body).wrap(Wrap { trim: true }),
            sections[0],
        );

        let summary_body = Text::from(vec![
            Line::from(Span::styled(
                self.tr("简介与评估", "Summary & Advice"),
                section_title_style(),
            )),
            Line::from(asset.summary.clone()),
            Line::from(vec![
                Span::styled(
                    format!("{} ", self.tr("评估建议", "Removal Advice")),
                    Style::default().fg(color_dim()),
                ),
                Span::styled(
                    removal_advice_label(asset.removal_advice, self.language),
                    badge_style(asset.removal_advice),
                ),
            ]),
            Line::from(format!(
                "{} {}",
                self.tr("评估说明", "Reason"),
                asset.advice_reason
            )),
        ]);
        frame.render_widget(
            Paragraph::new(summary_body)
                .style(Style::default().fg(color_text()))
                .wrap(Wrap { trim: true }),
            sections[1],
        );

        let meta_body = Text::from(vec![
            Line::from(Span::styled(
                self.tr("关键元数据", "Key Metadata"),
                section_title_style(),
            )),
            Line::from(format!("{} {}", self.tr("大小", "Size"), asset.size_label())),
            Line::from(format!(
                "{} {}",
                self.tr("最近使用", "Last Used"),
                asset.last_used_label()
            )),
            Line::from(format!(
                "{} {}",
                self.tr("可清缓存", "Cache Cleanable"),
                yes_no_localized(asset.cache_cleanable, self.language)
            )),
        ]);
        frame.render_widget(
            Paragraph::new(meta_body)
                .style(Style::default().fg(color_text()))
                .wrap(Wrap { trim: true }),
            sections[2],
        );

        let mut path_lines = vec![Line::from(Span::styled(
            self.tr("路径与安装信息", "Paths & Install Info"),
            section_title_style(),
        ))];
        append_multiline_lines(&mut path_lines, &asset.detail);
        if let Some(days) = removable_idle_days(asset) {
            path_lines.push(Line::from(""));
            path_lines.push(Line::from(Span::styled(
                self.tr("删除建议", "Removal Hint"),
                section_title_style(),
            )));
            path_lines.push(Line::from(Span::styled(
                if self.language.is_zh() {
                    format!("超过 {days} 天未被调用，可删除")
                } else {
                    format!("Unused for over {days} days and marked removable.")
                },
                Style::default()
                    .fg(color_warn())
                    .add_modifier(Modifier::BOLD),
            )));
        }
        frame.render_widget(
            Paragraph::new(Text::from(path_lines))
                .style(Style::default().fg(color_dim()))
                .wrap(Wrap { trim: true }),
            sections[3],
        );
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let block = panel_block(" ", Style::default().fg(color_cyan()));
        frame.render_widget(block, area);
        let inner = inner(area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);

        let keys = if self.language.is_zh() {
            "[↑↓] 移动  [0-9] 跳转  [/] 搜索   [Space] 选择  [Delete] 删除  [C] 清缓存   [R] 刷新  [S] 排序  [Esc] 返回/退出  [Enter] 确认"
        } else {
            "[↑↓] Move  [0-9] Jump  [/] Search   [Space] Select  [Delete] Remove  [C] Clean Cache   [R] Refresh  [S] Sort  [Esc] Back/Quit  [Enter] Confirm"
        };

        frame.render_widget(
            Paragraph::new(keys).style(Style::default().fg(color_dim())),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new(self.message.clone()).style(Style::default().fg(color_text())),
            rows[1],
        );
    }

    fn render_action_panel(&self, frame: &mut Frame, area: Rect) {
        let panel_style = action_panel_style(self.pending_intent, self.awaiting_password);
        let title = if let Some(running) = &self.running_action {
            format!(" {} · {} ", self.tr("动作控制台", "Action Console"), running.operation.label())
        } else if let Some(intent) = self.pending_intent {
            format!(" {} · {} ", self.tr("动作控制台", "Action Console"), action_panel_title(intent, self.language))
        } else if self.search_mode {
            format!(" {} · {} ", self.tr("动作控制台", "Action Console"), self.tr("搜索", "Search"))
        } else {
            format!(" {} ", self.tr("动作控制台", "Action Console"))
        };
        let block = panel_block(&title, panel_style).border_type(BorderType::Plain);
        frame.render_widget(block, area);

        let inner = inner(area);
        if let Some(running) = &self.running_action {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(2),
                    Constraint::Min(1),
                ])
                .split(inner);
            let ratio = if running.total == 0 {
                0.0
            } else {
                running.completed as f64 / running.total as f64
            };
            let summary = if self.language.is_zh() {
                format!(
                    "{} {}\n进度 {}/{}",
                    SPINNER[self.spinner_index],
                    running.label,
                    running.completed,
                    running.total
                )
            } else {
                format!(
                    "{} {}\nCompleted {}/{}",
                    SPINNER[self.spinner_index],
                    running.label,
                    running.completed,
                    running.total
                )
            };
            frame.render_widget(
                Paragraph::new(summary)
                    .style(Style::default().fg(color_text()))
                    .wrap(Wrap { trim: true }),
                rows[0],
            );
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio)
                    .label(format!("{:.0}%", ratio * 100.0))
                    .gauge_style(Style::default().fg(color_cyan()).bg(color_panel())),
                rows[2],
            );
            frame.render_widget(
                Paragraph::new(self.tr(
                    "执行期间会锁定输入；删除成功后只更新当前列表。",
                    "Input is locked during execution; successful deletes update the current list.",
                ))
                .style(Style::default().fg(color_dim()))
                .wrap(Wrap { trim: true }),
                rows[3],
            );
            return;
        }

        let body = if self.search_mode {
            if self.language.is_zh() {
                format!(
                    "当前搜索：{}\n实时过滤已开启。\nEnter 保留结果，Esc 清空并退出。",
                    if self.search_query.is_empty() {
                        "（空）"
                    } else {
                        &self.search_query
                    }
                )
            } else {
                format!(
                    "Current search: {}\nLive filtering is active.\nPress Enter to keep the result, Esc to clear and exit.",
                    if self.search_query.is_empty() {
                        "(empty)"
                    } else {
                        &self.search_query
                    }
                )
            }
        } else if self.awaiting_password {
            self.pending_intent_body(self.pending_intent.unwrap_or(PendingIntent::Action(OperationKind::Delete)))
        } else if let Some(intent) = self.pending_intent {
            self.pending_intent_body(intent)
        } else if self.selection_count() == 0 {
            self.tr(
                "先在左侧列表中选择项目。\n建议先搜索，再多选，然后执行删除或清缓存。",
                "Select items in the list first.\nA common flow is search, multi-select, then remove or clean cache.",
            )
        } else {
            self.tr(
                &format!(
                    "已选 {} 项。\n可立即按 Delete 删除，或按 C 清缓存。\n如需重新盘点，按 R；如需退出，按 Esc。",
                    self.selection_count()
                ),
                &format!(
                    "{} item(s) selected.\nPress Delete to remove, C to clean cache.\nPress R to refresh or Esc to quit.",
                    self.selection_count()
                ),
            )
        };

        frame.render_widget(
            Paragraph::new(body)
                .style(Style::default().fg(color_text()))
                .wrap(Wrap { trim: true }),
            inner,
        );
    }

    fn render_overlay(&self, frame: &mut Frame, intent: PendingIntent) {
        let style = action_panel_style(Some(intent), self.awaiting_password);
        let area = centered_rect(74, 10, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" PKGOH ", brand_style()),
                Span::raw(" "),
                Span::styled(action_panel_title(intent, self.language), style),
            ]))
            .borders(Borders::ALL)
            .border_style(style)
            .border_type(BorderType::Thick)
            .style(panel_style());
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(self.pending_intent_body(intent))
                .block(Block::default())
                .style(Style::default().fg(color_text()))
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true }),
            inner(area),
        );
    }

    fn render_running_overlay(&self, frame: &mut Frame) {
        let Some(running) = &self.running_action else {
            return;
        };

        let area = centered_rect(74, 10, frame.area());
        frame.render_widget(Clear, area);
        let style = action_panel_style(Some(PendingIntent::Action(running.operation)), false);
        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" PKGOH ", brand_style()),
                Span::raw(" "),
                Span::styled(
                    self.tr("执行中", "In Progress"),
                    style.add_modifier(Modifier::BOLD),
                ),
            ]))
            .borders(Borders::ALL)
            .border_style(style)
            .border_type(BorderType::Thick)
            .style(panel_style());
        frame.render_widget(block, area);

        let inner = inner(area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);
        let ratio = if running.total == 0 {
            0.0
        } else {
            running.completed as f64 / running.total as f64
        };

        let text = if self.language.is_zh() {
            format!(
                "{} {}\n正在处理：{}",
                SPINNER[self.spinner_index],
                running.operation.progress_label(),
                running.label
            )
        } else {
            format!(
                "{} {}\nProcessing: {}",
                SPINNER[self.spinner_index],
                running.operation.progress_label(),
                running.label
            )
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(color_text()))
                .wrap(Wrap { trim: true }),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new(if self.language.is_zh() {
                format!("进度 {}/{}", running.completed, running.total)
            } else {
                format!("Completed {}/{}", running.completed, running.total)
            })
            .style(Style::default().fg(color_dim())),
            rows[1],
        );
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .label(format!("{:.0}%", ratio * 100.0))
                .gauge_style(Style::default().fg(color_cyan()).bg(color_panel())),
            rows[2],
        );
        frame.render_widget(
            Paragraph::new(self.tr(
                "请稍候，窗口会在删除完成后自动展示结果。",
                "Please wait. This window will show the result automatically when the action finishes.",
            ))
            .style(Style::default().fg(color_dim()))
            .wrap(Wrap { trim: true }),
            rows[3],
        );
    }

    fn render_result_overlay(&self, frame: &mut Frame, feedback: &ActionFeedback) {
        let area = centered_rect(74, 9, frame.area());
        frame.render_widget(Clear, area);
        let has_failure = feedback.report.failed > 0;
        let style = if has_failure {
            Style::default().fg(color_warn()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color_cyan()).add_modifier(Modifier::BOLD)
        };
        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" PKGOH ", brand_style()),
                Span::raw(" "),
                Span::styled(
                    if self.language.is_zh() {
                        "操作结果"
                    } else {
                        "Action Result"
                    },
                    style,
                ),
            ]))
            .borders(Borders::ALL)
            .border_style(style)
            .border_type(BorderType::Thick)
            .style(panel_style());
        frame.render_widget(block, area);

        let remaining = 2.0_f32 - feedback.shown_at.elapsed().as_secs_f32();
        let remaining = remaining.max(0.0);
        let summary = if self.language.is_zh() {
            format!(
                "{}：成功 {}，失败 {}，共处理 {} 项。\n结果窗口将在 {:.1} 秒后自动关闭。",
                feedback.report.operation.success_label(),
                feedback.report.succeeded,
                feedback.report.failed,
                feedback.report.attempted,
                remaining
            )
        } else {
            format!(
                "{}: {} succeeded, {} failed, {} attempted.\nThis result window will close automatically in {:.1}s.",
                feedback.report.operation.success_label(),
                feedback.report.succeeded,
                feedback.report.failed,
                feedback.report.attempted,
                remaining
            )
        };
        let detail = if let Some(error) = feedback.report.outputs.iter().find(|output| !output.success) {
            if self.language.is_zh() {
                format!("首个失败：{} - {}", error.label, error.detail)
            } else {
                format!("First failure: {} - {}", error.label, error.detail)
            }
        } else if let Some(success) = feedback.report.outputs.iter().find(|output| output.success) {
            if self.language.is_zh() {
                format!("最近输出：{} - {}", success.label, success.detail)
            } else {
                format!("Latest output: {} - {}", success.label, success.detail)
            }
        } else {
            String::new()
        };

        frame.render_widget(
            Paragraph::new(format!("{summary}\n{detail}"))
                .style(Style::default().fg(color_text()))
                .wrap(Wrap { trim: true }),
            inner(area),
        );
    }

    fn current_status_text(&self) -> String {
        if let Some(running) = &self.running_action {
            return if self.language.is_zh() {
                format!("执行中 · {}", running.operation.label())
            } else {
                format!("Running · {}", running.operation.label())
            };
        }

        if self.awaiting_password {
            return self.tr("等待管理员权限", "Awaiting Admin Permission");
        }

        if let Some(intent) = self.pending_intent {
            return action_panel_title(intent, self.language).to_string();
        }

        if self.search_mode {
            return self.tr("搜索输入中", "Search Input").to_string();
        }

        if !self.search_query.is_empty() {
            return self.tr("已过滤列表", "Filtered View").to_string();
        }

        self.tr("资产控制台", "Asset Console")
    }

    fn current_status_style(&self) -> Style {
        if self.running_action.is_some() {
            return Style::default()
                .fg(color_cyan())
                .add_modifier(Modifier::BOLD);
        }
        if self.awaiting_password {
            return Style::default()
                .fg(color_warn())
                .add_modifier(Modifier::BOLD);
        }
        if let Some(intent) = self.pending_intent {
            return action_panel_style(Some(intent), false);
        }
        if self.search_mode || !self.search_query.is_empty() {
            return Style::default()
                .fg(color_cyan())
                .add_modifier(Modifier::BOLD);
        }

        Style::default().fg(color_dim())
    }

    fn pending_intent_body(&self, intent: PendingIntent) -> String {
        if self.awaiting_password {
            let masked = "*".repeat(self.password_input.chars().count());
            let hint = self.password_error.clone().unwrap_or_else(|| {
                self.tr(
                    "请输入管理员密码后按回车，Esc 取消。",
                    "Enter the admin password and press Enter, or Esc to cancel.",
                )
            });
            return if self.language.is_zh() {
                format!(
                    "{}\n管理员密码: {}\n{}",
                    action_panel_title(intent, self.language),
                    masked,
                    hint,
                )
            } else {
                format!(
                    "{}\nAdmin Password: {}\n{}",
                    action_panel_title(intent, self.language),
                    masked,
                    hint,
                )
            };
        }

        match intent {
            PendingIntent::Action(action) => {
                let selected_assets = self.selected_assets();
                let estimate = estimate_reclaim_bytes(action, &selected_assets);
                let core_count = selected_assets
                    .iter()
                    .filter(|asset| asset.removal_advice == RemovalAdvice::CoreDependency)
                    .count();
                let keep_count = selected_assets
                    .iter()
                    .filter(|asset| asset.removal_advice == RemovalAdvice::Keep)
                    .count();

                if self.language.is_zh() {
                    match action {
                        OperationKind::Delete => format!(
                            "{}\n将删除 {} 个选中项，预计释放 {}\n{}\n再次按 Delete 或 Enter 执行，Esc 取消。",
                            action_panel_title(intent, self.language),
                            selected_assets.len(),
                            human_size(estimate),
                            delete_risk_note_zh(core_count, keep_count),
                        ),
                        OperationKind::CleanCache => format!(
                            "{}\n将清理 {} 个选中项关联缓存，预计释放 {}\n共享缓存按来源只计算一次。\n再次按 C 或 Enter 执行，Esc 取消。",
                            action_panel_title(intent, self.language),
                            selected_assets.len(),
                            human_size(estimate),
                        ),
                    }
                } else {
                    match action {
                        OperationKind::Delete => format!(
                            "{}\nDelete {} selected item(s), estimated reclaim {}\n{}\nPress Delete again or Enter to run, Esc to cancel.",
                            action_panel_title(intent, self.language),
                            selected_assets.len(),
                            human_size(estimate),
                            delete_risk_note_en(core_count, keep_count),
                        ),
                        OperationKind::CleanCache => format!(
                            "{}\nClean caches for {} selected item(s), estimated reclaim {}\nShared caches are counted once per source.\nPress C again or Enter to run, Esc to cancel.",
                            action_panel_title(intent, self.language),
                            selected_assets.len(),
                            human_size(estimate),
                        ),
                    }
                }
            }
            PendingIntent::Refresh => {
                if self.language.is_zh() {
                    "刷新确认\n将重新扫描全部资产列表。\n再次按 R 或 Enter 执行，Esc 取消。".to_string()
                } else {
                    "Refresh Confirmation\nThis will rescan the full asset list.\nPress R again or Enter to run, Esc to cancel.".to_string()
                }
            }
            PendingIntent::Quit => {
                if self.language.is_zh() {
                    "退出确认\n将退出 pkgoh。\n再次按 Esc 或 Enter 退出，按 N 取消。".to_string()
                } else {
                    "Quit Confirmation\nThis will close pkgoh.\nPress Esc or Enter to quit, or N to cancel.".to_string()
                }
            }
        }
    }

    fn tr(&self, zh: &str, en: &str) -> String {
        if self.language.is_zh() {
            zh.to_string()
        } else {
            en.to_string()
        }
    }
}

fn color_bg() -> Color {
    Color::Rgb(13, 21, 22)
}

fn color_panel() -> Color {
    Color::Rgb(18, 28, 30)
}

fn color_cyan() -> Color {
    Color::Rgb(0, 229, 255)
}

fn color_warn() -> Color {
    Color::Rgb(255, 171, 0)
}

fn color_danger() -> Color {
    Color::Rgb(255, 82, 82)
}

fn color_text() -> Color {
    Color::Rgb(224, 224, 224)
}

fn color_dim() -> Color {
    Color::Rgb(117, 117, 117)
}

fn panel_style() -> Style {
    Style::default().bg(color_panel()).fg(color_text())
}

fn brand_style() -> Style {
    Style::default()
        .fg(color_bg())
        .bg(color_cyan())
        .add_modifier(Modifier::BOLD)
}

fn section_title_style() -> Style {
    Style::default()
        .fg(color_cyan())
        .add_modifier(Modifier::BOLD)
}

fn panel_block<'a>(title: &'a str, border_style: Style) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(panel_style())
}

fn badge_style(advice: RemovalAdvice) -> Style {
    advice_style(advice).add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

fn visible_window(cursor: usize, total: usize, max_rows: usize) -> (usize, usize) {
    if total <= max_rows {
        return (0, total);
    }

    let start = cursor
        .saturating_sub(max_rows / 2)
        .min(total.saturating_sub(max_rows));
    (start, (start + max_rows).min(total))
}

fn highlight_query(text: &str, query: &str, is_focused: bool, highlight: Color) -> Text<'static> {
    if query.is_empty() || is_focused {
        return Text::from(Line::from(text.to_string()));
    }

    let text_lower = text.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    let Some(index) = text_lower.find(&query_lower) else {
        return Text::from(Line::from(text.to_string()));
    };
    let end = index + query.len().min(text.len().saturating_sub(index));

    Text::from(Line::from(vec![
        Span::raw(text[..index].to_string()),
        Span::styled(
            text[index..end].to_string(),
            Style::default()
                .fg(highlight)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ),
        Span::raw(text[end..].to_string()),
    ]))
}

fn detail_tags(asset: &Asset, config: &Config, language: Language) -> Line<'static> {
    let mut spans = Vec::new();

    if asset.is_large(&config.highlight) {
        spans.push(Span::styled(
            if language.is_zh() { "[大体积]" } else { "[Large]" },
            Style::default().fg(color_warn()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }

    if asset.is_stale(&config.highlight) {
        spans.push(Span::styled(
            if language.is_zh() { "[长期未用]" } else { "[Stale]" },
            Style::default().fg(color_danger()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }

    if spans.is_empty() {
        spans.push(Span::styled(
            if language.is_zh() { "[正常]" } else { "[Normal]" },
            Style::default().fg(color_dim()),
        ));
    }

    Line::from(spans)
}

fn removable_idle_days(asset: &Asset) -> Option<i64> {
    if asset.removal_advice != RemovalAdvice::Removable {
        return None;
    }

    let days = chrono::Local::now()
        .signed_duration_since(asset.last_used)
        .num_days();

    if days > 180 {
        Some(days)
    } else {
        None
    }
}

fn action_message(report: &ActionReport, language: Language) -> String {
    let summary = if language.is_zh() {
        format!(
            "{}：成功 {}，失败 {}，共处理 {} 项。",
            report.operation.success_label(),
            report.succeeded,
            report.failed,
            report.attempted,
        )
    } else {
        format!(
            "{}: {} succeeded, {} failed, {} attempted.",
            report.operation.success_label(),
            report.succeeded,
            report.failed,
            report.attempted,
        )
    };

    if let Some(first_error) = report.outputs.iter().find(|output| !output.success) {
        if language.is_zh() {
            format!("{} 首个失败：{} - {}。", summary, first_error.label, first_error.detail)
        } else {
            format!("{} First failure: {} - {}.", summary, first_error.label, first_error.detail)
        }
    } else if let Some(first_success) = report.outputs.iter().find(|output| output.success) {
        if language.is_zh() {
            format!("{} 最近输出：{} - {}。", summary, first_success.label, first_success.detail)
        } else {
            format!("{} Latest output: {} - {}.", summary, first_success.label, first_success.detail)
        }
    } else {
        summary
    }
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn yes_no_localized(value: bool, language: Language) -> &'static str {
    match (value, language.is_zh()) {
        (true, true) => "是",
        (false, true) => "否",
        (true, false) => "yes",
        (false, false) => "no",
    }
}

fn removal_advice_label(advice: RemovalAdvice, language: Language) -> &'static str {
    match (advice, language.is_zh()) {
        (RemovalAdvice::Removable, true) => "可删除",
        (RemovalAdvice::Keep, true) => "建议保留",
        (RemovalAdvice::CoreDependency, true) => "核心依赖",
        (RemovalAdvice::Removable, false) => "Removable",
        (RemovalAdvice::Keep, false) => "Keep Recommended",
        (RemovalAdvice::CoreDependency, false) => "Core Dependency",
    }
}

fn action_panel_title(intent: PendingIntent, language: Language) -> &'static str {
    match (intent, language.is_zh()) {
        (PendingIntent::Action(OperationKind::Delete), true) => "删除确认",
        (PendingIntent::Action(OperationKind::CleanCache), true) => "清缓存确认",
        (PendingIntent::Refresh, true) => "刷新确认",
        (PendingIntent::Quit, true) => "退出确认",
        (PendingIntent::Action(OperationKind::Delete), false) => "Delete Confirmation",
        (PendingIntent::Action(OperationKind::CleanCache), false) => "Cache Cleanup Confirmation",
        (PendingIntent::Refresh, false) => "Refresh Confirmation",
        (PendingIntent::Quit, false) => "Quit Confirmation",
    }
}

fn advice_style(advice: RemovalAdvice) -> Style {
    match advice {
        RemovalAdvice::Removable => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        RemovalAdvice::Keep => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        RemovalAdvice::CoreDependency => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn action_panel_style(intent: Option<PendingIntent>, awaiting_password: bool) -> Style {
    if awaiting_password {
        return match intent {
            Some(PendingIntent::Action(OperationKind::Delete)) => {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            }
            Some(PendingIntent::Action(OperationKind::CleanCache)) => {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            }
            Some(PendingIntent::Refresh) => {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            }
            Some(PendingIntent::Quit) => {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            }
            None => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        };
    }

    match intent {
        Some(PendingIntent::Action(OperationKind::Delete)) => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        Some(PendingIntent::Action(OperationKind::CleanCache)) => {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        }
        Some(PendingIntent::Refresh) => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Some(PendingIntent::Quit) => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        None => Style::default().fg(Color::White),
    }
}

fn append_multiline_lines(lines: &mut Vec<Line<'static>>, content: &str) {
    if content.is_empty() {
        lines.push(Line::from(""));
        return;
    }

    for line in content.lines() {
        lines.push(Line::from(line.to_string()));
    }
}

fn delete_risk_note_zh(core_count: usize, keep_count: usize) -> String {
    if core_count > 0 {
        format!("注意：其中 {core_count} 项被评估为核心依赖，删除后很可能导致其他工具报错。")
    } else if keep_count > 0 {
        format!("注意：其中 {keep_count} 项被评估为建议保留，删除后可能需要手动修复环境。")
    } else {
        "评估结果：当前选中项都属于可删除级别。".to_string()
    }
}

fn delete_risk_note_en(core_count: usize, keep_count: usize) -> String {
    if core_count > 0 {
        format!(
            "Warning: {core_count} selected item(s) are marked as core dependencies and may break other installed tools."
        )
    } else if keep_count > 0 {
        format!(
            "Warning: {keep_count} selected item(s) are marked as keep recommended and may need manual fixes afterward."
        )
    } else {
        "Assessment: all selected items are currently in the removable tier.".to_string()
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(width.min(area.width.saturating_sub(2))),
            Constraint::Fill(1),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height.min(area.height.saturating_sub(2))),
            Constraint::Fill(1),
        ])
        .split(horizontal[1])[1]
}

fn inner(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

fn enabled_source_count(config: &Config) -> usize {
    let toggles = [
        config.sources.brew,
        config.sources.npm,
        config.sources.pnpm,
        config.sources.cargo,
        config.sources.pip,
        config.sources.uv,
        config.sources.mas,
    ];
    let enabled = toggles.into_iter().filter(|value| *value).count();
    if enabled == 0 {
        SourceKind::all().len()
    } else {
        enabled
    }
}
