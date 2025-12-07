use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph},
};
use rodio::{Decoder, OutputStream, Sink};
use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct AudioPlayer {
    _stream: OutputStream,
    sink: Arc<Mutex<Sink>>,
}

impl AudioPlayer {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;
        Ok(Self {
            _stream: stream,
            sink: Arc::new(Mutex::new(sink)),
        })
    }

    fn play(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let sink = self.sink.lock().unwrap();
        sink.stop();

        let file = File::open(path)?;
        let source = Decoder::new(BufReader::new(file))?;
        sink.append(source);
        sink.play();
        Ok(())
    }

    fn toggle_pause(&self) {
        let sink = self.sink.lock().unwrap();
        if sink.is_paused() {
            sink.play();
        } else {
            sink.pause();
        }
    }

    fn is_paused(&self) -> bool {
        self.sink.lock().unwrap().is_paused()
    }

    fn is_empty(&self) -> bool {
        self.sink.lock().unwrap().empty()
    }

    fn get_position(&self) -> Duration {
        // Rodio non fornisce direttamente la posizione, usiamo un timer interno
        Duration::from_secs(0)
    }
}

struct App {
    current_dir: PathBuf,
    items: Vec<PathBuf>,
    list_state: ListState,
    selected_track: Option<PathBuf>,
    selected_track_name: Option<String>,
    audio_player: AudioPlayer,
    is_playing: bool,
    current_time: Duration,
    total_time: Duration,
    playback_start: Option<Instant>,
    waveform: Vec<f32>,
    error_message: Option<String>,
}

impl App {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let current_dir = std::env::current_dir()?;
        let audio_player = AudioPlayer::new()?;

        let mut app = App {
            current_dir: current_dir.clone(),
            items: Vec::new(),
            list_state: ListState::default(),
            selected_track: None,
            selected_track_name: None,
            audio_player,
            is_playing: false,
            current_time: Duration::from_secs(0),
            total_time: Duration::from_secs(180),
            playback_start: None,
            waveform: vec![0.0; 50],
            error_message: None,
        };
        app.load_directory()?;
        app.list_state.select(Some(0));
        Ok(app)
    }

    fn load_directory(&mut self) -> io::Result<()> {
        self.items.clear();

        if self.current_dir.parent().is_some() {
            self.items.push(PathBuf::from(".."));
        }

        let entries = fs::read_dir(&self.current_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.items.push(path);
            } else if let Some(ext) = path.extension() {
                let ext = ext.to_str().unwrap_or("").to_lowercase();
                if ["mp3", "flac", "wav", "ogg", "m4a", "opus"].contains(&ext.as_str()) {
                    self.items.push(path);
                }
            }
        }

        self.items.sort();
        Ok(())
    }

    fn next(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn select_item(&mut self) -> io::Result<()> {
        if let Some(i) = self.list_state.selected() {
            if i < self.items.len() {
                let path = &self.items[i];

                if path.file_name() == Some(std::ffi::OsStr::new("..")) {
                    if let Some(parent) = self.current_dir.parent() {
                        self.current_dir = parent.to_path_buf();
                        self.load_directory()?;
                        self.list_state.select(Some(0));
                    }
                } else if path.is_dir() {
                    self.current_dir = path.clone();
                    self.load_directory()?;
                    self.list_state.select(Some(0));
                } else {
                    // Riproduci traccia audio
                    match self.audio_player.play(path) {
                        Ok(_) => {
                            self.selected_track = Some(path.clone());
                            self.selected_track_name = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|s| s.to_string());
                            self.is_playing = true;
                            self.current_time = Duration::from_secs(0);
                            self.playback_start = Some(Instant::now());
                            self.generate_waveform();
                            self.error_message = None;
                        }
                        Err(e) => {
                            self.error_message = Some(format!("Errore riproduzione: {}", e));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn toggle_playback(&mut self) {
        if self.selected_track.is_some() {
            self.audio_player.toggle_pause();
            self.is_playing = !self.audio_player.is_paused();

            if self.is_playing {
                self.playback_start = Some(Instant::now() - self.current_time);
            }
        }
    }

    fn update_playback(&mut self) {
        if self.is_playing && self.playback_start.is_some() {
            let elapsed = self.playback_start.unwrap().elapsed();
            self.current_time = elapsed;

            if self.current_time > self.total_time {
                self.current_time = self.total_time;
            }

            // Aggiorna waveform animato
            self.waveform.rotate_left(1);
            let t = self.current_time.as_secs_f32();
            let len = self.waveform.len();
            self.waveform[len - 1] =
                (t * 2.0).sin() * 0.5 + (t * 5.0).cos() * 0.3 + (t * 0.5).sin() * 0.2;
        }

        // Controlla se la riproduzione è finita
        if self.is_playing && self.audio_player.is_empty() {
            self.is_playing = false;
            self.current_time = Duration::from_secs(0);
            self.playback_start = None;
        }
    }

    fn generate_waveform(&mut self) {
        use std::f32::consts::PI;
        for i in 0..self.waveform.len() {
            let t = i as f32 / self.waveform.len() as f32;
            self.waveform[i] = (t * PI * 4.0).sin() * 0.5 + (t * PI * 8.0).cos() * 0.25;
        }
    }

    fn format_duration(duration: Duration) -> String {
        let secs = duration.as_secs();
        let mins = secs / 60;
        let secs = secs % 60;
        format!("{:02}:{:02}", mins, secs)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new()?;
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err)
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        app.update_playback();
        terminal.draw(|f| ui(f, app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Down | KeyCode::Char('j') => app.next(),
                    KeyCode::Up | KeyCode::Char('k') => app.previous(),
                    KeyCode::Enter => app.select_item()?,
                    KeyCode::Char(' ') => app.toggle_playback(),
                    _ => {}
                }
            }
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(f.area());

    render_file_browser(f, app, chunks[0]);
    render_player_info(f, app, chunks[1]);
}

fn render_file_browser(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .items
        .iter()
        .map(|path| {
            let name = if path.file_name() == Some(std::ffi::OsStr::new("..")) {
                "📁 ..".to_string()
            } else if path.is_dir() {
                format!(
                    "📁 {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                )
            } else {
                format!(
                    "🎵 {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                )
            };
            ListItem::new(name)
        })
        .collect();

    let title = format!(" 📂 {} ", app.current_dir.display());
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_player_info(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(5),
        ])
        .split(area);

    // Titolo traccia
    let track_name = app
        .selected_track_name
        .as_deref()
        .unwrap_or("Nessuna traccia selezionata");
    let title = Paragraph::new(track_name)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 🎵 Traccia Corrente ")
                .style(Style::default().fg(Color::Green)),
        )
        .style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(title, chunks[0]);

    // Barra di progresso
    let progress = if app.total_time.as_secs() > 0 {
        (app.current_time.as_secs_f64() / app.total_time.as_secs_f64() * 100.0).min(100.0) as u16
    } else {
        0
    };

    let time_label = format!(
        "{} / {}",
        App::format_duration(app.current_time),
        App::format_duration(app.total_time)
    );

    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" ⏱️  Progresso "),
        )
        .gauge_style(Style::default().fg(Color::Yellow).bg(Color::Black))
        .percent(progress)
        .label(time_label);
    f.render_widget(gauge, chunks[1]);

    // Visualizzazione waveform
    render_waveform(f, app, chunks[2]);

    // Controlli ed errori
    let status = if app.is_playing {
        "▶️  Playing"
    } else if app.selected_track.is_some() {
        "⏸️  Paused"
    } else {
        "⏹️  Stopped"
    };

    let mut lines = vec![
        Line::from(vec![Span::styled(
            status,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from("Controls: [Space] Play/Pause | [↑↓/jk] Navigate | [Enter] Select | [Q] Quit"),
    ];

    if let Some(error) = &app.error_message {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            format!("⚠️  {}", error),
            Style::default().fg(Color::Red),
        )]));
    }

    let controls = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 🎮 Controlli ")
            .style(Style::default().fg(Color::Magenta)),
    );
    f.render_widget(controls, chunks[3]);
}

fn render_waveform(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 📊 Visualizzazione ")
        .style(Style::default().fg(Color::Blue));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let width = inner.width as usize;

    let mut lines = vec![String::new(); height];

    for x in 0..width.min(app.waveform.len()) {
        let value = app.waveform[x];
        let y = ((1.0 - value) * (height as f32 / 2.0)) as usize;
        let y = y.min(height - 1);

        if x < lines[y].len() {
            continue;
        }

        while lines[y].len() < x {
            lines[y].push(' ');
        }

        let char = if app.is_playing { '█' } else { '▓' };
        lines[y].push(char);
    }

    for (i, line) in lines.iter().enumerate() {
        let color = if i < height / 3 {
            Color::Red
        } else if i < 2 * height / 3 {
            Color::Yellow
        } else {
            Color::Green
        };

        let paragraph = Paragraph::new(line.as_str()).style(Style::default().fg(color));

        let line_area = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };

        f.render_widget(paragraph, line_area);
    }
}
