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
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct AudioPlayer {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Arc<Mutex<Option<Sink>>>,
    volume: f32,
}

impl AudioPlayer {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (_stream, stream_handle) = OutputStream::try_default()
            .map_err(|e| format!("Errore inizializzazione audio: {}", e))?;
        Ok(Self {
            _stream,
            stream_handle,
            sink: Arc::new(Mutex::new(None)),
            volume: 0.5,
        })
    }

    fn play(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let mut sink_lock = self.sink.lock().unwrap();

        if let Some(old_sink) = sink_lock.take() {
            old_sink.stop();
        }

        let sink = Sink::try_new(&self.stream_handle)
            .map_err(|e| format!("Errore creazione sink: {}", e))?;

        sink.set_volume(self.volume);

        let file = File::open(path)?;
        let source = Decoder::new(BufReader::new(file))?;
        sink.append(source);
        sink.play();

        *sink_lock = Some(sink);
        Ok(())
    }

    fn toggle_pause(&self) {
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            if sink.is_paused() {
                sink.play();
            } else {
                sink.pause();
            }
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink.set_volume(self.volume);
        }
    }

    fn increase_volume(&mut self) {
        self.set_volume(self.volume + 0.05);
    }

    fn decrease_volume(&mut self) {
        self.set_volume(self.volume - 0.05);
    }

    fn get_volume(&self) -> f32 {
        self.volume
    }

    fn is_paused(&self) -> bool {
        self.sink
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.is_paused())
            .unwrap_or(true)
    }

    fn is_empty(&self) -> bool {
        self.sink
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.empty())
            .unwrap_or(true)
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
    histogram: Vec<f32>,
    animation_phase: f32,
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
            histogram: vec![0.1; 32],
            animation_phase: 0.0,
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

            // Aggiorna fase animazione
            self.animation_phase += 0.15;

            // Genera istogramma reattivo simulato
            self.update_histogram();
        }

        if self.is_playing && self.audio_player.is_empty() {
            self.is_playing = false;
            self.current_time = Duration::from_secs(0);
            self.playback_start = None;
        }
    }

    fn update_histogram(&mut self) {
        use std::f32::consts::PI;
        let t = self.current_time.as_secs_f32();

        for i in 0..self.histogram.len() {
            let freq = (i as f32 / self.histogram.len() as f32) * 10.0;

            // Simula diverse frequenze con onde sinusoidali
            let bass = ((t * 2.0 + self.animation_phase).sin() * 0.3).abs();
            let mid = ((t * 4.0 + freq + self.animation_phase).sin() * 0.4).abs();
            let high = ((t * 8.0 + freq * 2.0 + self.animation_phase).cos() * 0.3).abs();

            // Aggiungi variazione casuale per effetto più realistico
            let noise = ((t * 13.7 + i as f32).sin() * 0.15).abs();

            let target = if i < 8 {
                bass + noise
            } else if i < 24 {
                mid + noise
            } else {
                high + noise
            };

            // Smooth interpolation
            self.histogram[i] = self.histogram[i] * 0.7 + target * 0.3;
            self.histogram[i] = self.histogram[i].clamp(0.05, 1.0);
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
                    KeyCode::Char('+') | KeyCode::Char('=') => app.audio_player.increase_volume(),
                    KeyCode::Char('-') | KeyCode::Char('_') => app.audio_player.decrease_volume(),
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
            Constraint::Length(3),
            Constraint::Min(8),
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

    // Controllo volume
    render_volume_control(f, app, chunks[2]);

    // Visualizzazione istogramma
    render_histogram(f, app, chunks[3]);

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
        Line::from("Controls: [Space] Play/Pause | [↑↓/jk] Navigate | [Enter] Select"),
        Line::from("          [+/-] Volume | [Q] Quit"),
    ];

    if let Some(error) = &app.error_message {
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
    f.render_widget(controls, chunks[4]);
}

fn render_volume_control(f: &mut Frame, app: &App, area: Rect) {
    let volume_percent = (app.audio_player.get_volume() * 100.0) as u16;
    let volume_icon = if volume_percent == 0 {
        "🔇"
    } else if volume_percent < 33 {
        "🔈"
    } else if volume_percent < 66 {
        "🔉"
    } else {
        "🔊"
    };

    let volume_label = format!("{} {}%", volume_icon, volume_percent);

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" 🔊 Volume "))
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .percent(volume_percent)
        .label(volume_label);
    f.render_widget(gauge, area);
}

fn render_histogram(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 📊 Visualizzazione Audio ")
        .style(Style::default().fg(Color::Blue));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 2 {
        return;
    }

    let bar_width = (inner.width as usize / app.histogram.len()).max(1);
    let height = inner.height as usize;

    for (i, &amplitude) in app.histogram.iter().enumerate() {
        let bar_height = (amplitude * height as f32) as usize;
        let bar_height = bar_height.min(height);

        let x_pos = inner.x + (i * bar_width) as u16;

        if x_pos >= inner.x + inner.width {
            break;
        }

        for y in 0..bar_height {
            let y_pos = inner.y + inner.height - 1 - y as u16;

            // Colore basato sull'altezza (gradient)
            let color = if y > height * 2 / 3 {
                Color::Red
            } else if y > height / 3 {
                Color::Yellow
            } else {
                Color::Green
            };

            let bar_char = if app.is_playing { "█" } else { "▓" };

            let bar = Paragraph::new(
                bar_char.repeat(bar_width.min((inner.width - (x_pos - inner.x)) as usize)),
            )
            .style(Style::default().fg(color));

            let bar_area = Rect {
                x: x_pos,
                y: y_pos,
                width: bar_width.min((inner.x + inner.width - x_pos) as usize) as u16,
                height: 1,
            };

            f.render_widget(bar, bar_area);
        }
    }
}
