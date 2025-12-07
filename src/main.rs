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
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph},
};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use rustfft::{FftPlanner, num_complex::Complex};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{self, BufReader},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

// Wrapper per catturare i campioni audio
// agisce come un wrapper per una sorgente audio
// cattura i campioni audio (f32) utilizzando un Arc<Mutex>
// imlementa il trait Iterator e Source  di rodio
// per intercettare i campioni audio prima che
// raggiungano il sink audio
// buffer di 8192 per l'analisi in tempo reale
struct SampleCapturer<I> {
    input: I,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    max_size: usize,
}
impl<I> SampleCapturer<I> {
    // creamo un nuovo capturer che salva i campioni in un buffer condiviso
    fn new(input: I, buffer: Arc<Mutex<VecDeque<f32>>>) -> Self {
        Self {
            input,
            buffer,
            // dimensione massima del buffer
            max_size: 8192,
        }
    }
}
//
impl<I> Iterator for SampleCapturer<I>
where
    I: Source<Item = f32>,
{
    type Item = f32;
    // propaga il prossimo campione e contemporaneamente lo salva nel buffer
    fn next(&mut self) -> Option<f32> {
        if let Some(sample) = self.input.next() {
            let mut buffer = self.buffer.lock().unwrap();
            if buffer.len() >= self.max_size {
                // rimuoviamo il piu vecchio
                // per mantenere la dimensione fissa
                buffer.pop_front();
            }
            buffer.push_back(sample);
            Some(sample)
        } else {
            None
        }
    }
}

impl<I> Source for SampleCapturer<I>
where
    I: Source<Item = f32>,
{
    // implementazione per l'utilizzo di Source in Rodio
    fn current_frame_len(&self) -> Option<usize> {
        self.input.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.input.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.input.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }
}

///gestore per riproduzione e audio con supporto a :
/// - Riproduzione (mp3 - flac - wav , ecc
/// - controllo volume  
/// - cattura campioni per analisi spettrale in real time
/// - buffer condiviso per visualizzare FFT  
struct AudioPlayer {
    _stream: OutputStream,                   // per tutta la durata del programma
    stream_handle: OutputStreamHandle,       // usato per creare nuovi sink
    sink: Option<Sink>,                      // sink corrente permette stop, play e pause
    volume: f32,                             // volume 0.0 a 1.0
    audio_buffer: Arc<Mutex<VecDeque<f32>>>, // per analisi del brano
    sample_rate: u32,
    is_playing: Arc<Mutex<bool>>,     // flag per l'utilizzo esterno
    total_duration: Option<Duration>, // per la durata totale del brano
}

impl AudioPlayer {
    // inizializza il dispositivo audio (rodio)
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (_stream, stream_handle) = OutputStream::try_default()
            .map_err(|e| format!("Errore inizializzazione audio: {}", e))?;
        Ok(Self {
            _stream,
            stream_handle,
            sink: None,
            volume: 0.5,
            audio_buffer: Arc::new(Mutex::new(VecDeque::new())),
            sample_rate: 44100,
            is_playing: Arc::new(Mutex::new(false)),
            total_duration: None,
        })
    }
    // riproduce il file audio dal percorso (path) specificato
    fn play(&mut self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        // Ferma e rimuovi il sink precedente
        if let Some(old_sink) = self.sink.take() {
            old_sink.stop();
        }

        *self.is_playing.lock().unwrap() = false;
        self.audio_buffer.lock().unwrap().clear();

        // Crea nuovo sink per la riproduzione
        let sink = Sink::try_new(&self.stream_handle)
            .map_err(|e| format!("Errore creazione sink: {}", e))?;

        let file = File::open(path)?;
        let source = Decoder::new(BufReader::new(file))?;

        // informazioni sul file come la durata totale
        self.sample_rate = source.sample_rate();
        self.total_duration = source.total_duration();

        // Converti in f32 e cattura campioni per il buffer condiviso
        let source = source.convert_samples::<f32>();
        let capturer = SampleCapturer::new(source, self.audio_buffer.clone());

        // Applica volume
        let source = capturer.amplify(self.volume);

        // Aggiungi al sink e riproduci
        sink.append(source);
        sink.play();

        self.sink = Some(sink);
        *self.is_playing.lock().unwrap() = true;

        Ok(())
    }

    // funzione per il settaggio del volume (0.0 a 1.0)
    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
    }
    // aumenta il volume dello 0.05 (5%)
    fn increase_volume(&mut self) {
        self.set_volume(self.volume + 0.05);
    }
    // decrementa come sopra
    fn decrease_volume(&mut self) {
        self.set_volume(self.volume - 0.05);
    }
    // restituisce lo stato del volume
    fn get_volume(&self) -> f32 {
        self.volume
    }
    // controllo della riproduzione
    fn is_playing(&self) -> bool {
        if let Some(sink) = &self.sink {
            !sink.empty()
        } else {
            false
        }
    }
    // ferma la riproduzione
    fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        *self.is_playing.lock().unwrap() = false;
    }
    // restituisce la durata del brano corrente
    fn get_total_duration(&self) -> Option<Duration> {
        self.total_duration
    }
    // Ottiene i campioni audio
    fn get_audio_samples(&self, count: usize) -> Vec<f32> {
        let buffer = self.audio_buffer.lock().unwrap();
        buffer.iter().rev().take(count).copied().collect()
    }

    fn get_sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

// interfaccia utente e logica di controllo
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
    fft_planner: FftPlanner<f32>,
    error_message: Option<String>,
    continuous_play: bool,
    current_track_index: Option<usize>,
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
            total_time: Duration::from_secs(0),
            playback_start: None,
            histogram: vec![0.1; 32],
            fft_planner: FftPlanner::new(),
            error_message: None,
            continuous_play: false,
            current_track_index: None,
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
                    self.play_track_at_index(i);
                }
            }
        }
        Ok(())
    }

    fn play_track_at_index(&mut self, index: usize) {
        if index < self.items.len() {
            let path = &self.items[index];
            if !path.is_dir() && path.file_name() != Some(std::ffi::OsStr::new("..")) {
                match self.audio_player.play(path) {
                    Ok(_) => {
                        self.selected_track = Some(path.clone());
                        self.selected_track_name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(|s| s.to_string());
                        self.current_track_index = Some(index);
                        self.is_playing = true;
                        self.current_time = Duration::from_secs(0);
                        self.total_time = self
                            .audio_player
                            .get_total_duration()
                            .unwrap_or(Duration::from_secs(180));
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

    fn play_next_track(&mut self) {
        if let Some(current_idx) = self.current_track_index {
            // Trova il prossimo file audio
            for i in (current_idx + 1)..self.items.len() {
                let path = &self.items[i];
                if !path.is_dir() && path.file_name() != Some(std::ffi::OsStr::new("..")) {
                    self.play_track_at_index(i);
                    return;
                }
            }
            // Se siamo alla fine, ricomincia dall'inizio se continuous_play è attivo
            if self.continuous_play {
                for i in 0..current_idx {
                    let path = &self.items[i];
                    if !path.is_dir() && path.file_name() != Some(std::ffi::OsStr::new("..")) {
                        self.play_track_at_index(i);
                        return;
                    }
                }
            }
        }
        // Nessun brano successivo trovato
        self.is_playing = false;
    }

    fn play_previous_track(&mut self) {
        if let Some(current_idx) = self.current_track_index {
            // Trova il precedente file audio
            if current_idx > 0 {
                for i in (0..current_idx).rev() {
                    let path = &self.items[i];
                    if !path.is_dir() && path.file_name() != Some(std::ffi::OsStr::new("..")) {
                        self.play_track_at_index(i);
                        return;
                    }
                }
            }
        }
    }

    fn toggle_continuous_play(&mut self) {
        self.continuous_play = !self.continuous_play;
    }

    fn toggle_playback(&mut self) {
        if self.selected_track.is_some() {
            if self.is_playing {
                self.audio_player.stop();
                self.is_playing = false;
            } else {
                // Riavvia riproduzione
                if let Some(track) = self.selected_track.clone() {
                    let _ = self.audio_player.play(&track);
                    self.is_playing = true;
                    self.playback_start = Some(Instant::now());
                }
            }
        }
    }

    fn update_playback(&mut self) {
        let was_playing = self.is_playing;
        self.is_playing = self.audio_player.is_playing();

        // Se il brano è finito e continuous_play è attivo, riproduci il prossimo
        if was_playing && !self.is_playing && self.continuous_play {
            self.play_next_track();
        }

        if self.is_playing && self.playback_start.is_some() {
            let elapsed = self.playback_start.unwrap().elapsed();
            self.current_time = elapsed;

            if self.current_time > self.total_time {
                self.current_time = self.total_time;
            }

            // Analizza audio in tempo reale
            self.analyze_audio();
        } else if !self.is_playing {
            // Decay graduale quando non sta suonando
            for val in self.histogram.iter_mut() {
                *val *= 0.9;
                if *val < 0.05 {
                    *val = 0.05;
                }
            }
        }
    }

    fn analyze_audio(&mut self) {
        const FFT_SIZE: usize = 2048;
        let samples = self.audio_player.get_audio_samples(FFT_SIZE);

        if samples.len() < FFT_SIZE {
            return;
        }

        // Prepara buffer FFT
        let mut buffer: Vec<Complex<f32>> = samples[..FFT_SIZE]
            .iter()
            .map(|&s| Complex::new(s, 0.0))
            .collect();

        // Applica finestra di Hann per ridurre artefatti
        for (i, sample) in buffer.iter_mut().enumerate() {
            let window =
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos());
            *sample *= window;
        }

        // Esegui FFT
        let fft = self.fft_planner.plan_fft_forward(FFT_SIZE);
        fft.process(&mut buffer);

        // Converti in magnitudini e mappa alle barre
        let num_bars = self.histogram.len();
        let sample_rate = self.audio_player.get_sample_rate() as f32;
        let freq_per_bin = sample_rate / FFT_SIZE as f32;

        // Definisci bande di frequenza (logaritmiche)
        let min_freq: f32 = 60.0; // Aumentato da 20Hz per evitare rumori bassi
        let max_freq: f32 = 16000.0; // Ridotto da 20kHz

        // Trova la magnitudine massima per normalizzazione adattiva
        let mut max_magnitude = 0.0f32;

        for i in 0..num_bars {
            let t = i as f32 / num_bars as f32;
            let freq_ratio = (max_freq / min_freq).powf(t);
            let freq_start = min_freq * freq_ratio;
            let freq_ratio_end = (max_freq / min_freq).powf((i + 1) as f32 / num_bars as f32);
            let freq_end = min_freq * freq_ratio_end;

            let bin_start = (freq_start / freq_per_bin) as usize;
            let bin_end = ((freq_end / freq_per_bin).min((FFT_SIZE / 2) as f32)) as usize;

            // Calcola magnitudine media per questa banda
            let mut magnitude = 0.0;
            let mut count = 0;

            for bin in bin_start..bin_end {
                if bin < buffer.len() {
                    let mag =
                        (buffer[bin].re * buffer[bin].re + buffer[bin].im * buffer[bin].im).sqrt();
                    magnitude += mag;
                    count += 1;
                }
            }

            if count > 0 {
                magnitude /= count as f32;
                max_magnitude = max_magnitude.max(magnitude);
            }
        }

        // Normalizzazione adattiva
        let normalization_factor = if max_magnitude > 0.0 {
            1.0 / max_magnitude
        } else {
            1.0
        };

        // Seconda passata per aggiornare le barre
        for i in 0..num_bars {
            let t = i as f32 / num_bars as f32;
            let freq_ratio = (max_freq / min_freq).powf(t);
            let freq_start = min_freq * freq_ratio;
            let freq_ratio_end = (max_freq / min_freq).powf((i + 1) as f32 / num_bars as f32);
            let freq_end = min_freq * freq_ratio_end;

            let bin_start = (freq_start / freq_per_bin) as usize;
            let bin_end = ((freq_end / freq_per_bin).min((FFT_SIZE / 2) as f32)) as usize;

            let mut magnitude = 0.0;
            let mut count = 0;

            for bin in bin_start..bin_end {
                if bin < buffer.len() {
                    magnitude +=
                        (buffer[bin].re * buffer[bin].re + buffer[bin].im * buffer[bin].im).sqrt();
                    count += 1;
                }
            }

            if count > 0 {
                magnitude /= count as f32;

                // Normalizza con fattore adattivo
                magnitude *= normalization_factor;

                // SENSIBILITÀ: Scala finale (riduci per meno reattività)
                magnitude *= 0.8;

                // COMPRESSIONE: Comprimi dinamica
                magnitude = magnitude.powf(0.7);

                // Clamp prima dello smoothing
                magnitude = magnitude.clamp(0.0, 1.0);

                // SMOOTHING: Interpolazione fluida
                let smoothing = 0.7;
                self.histogram[i] = self.histogram[i] * smoothing + magnitude * (1.0 - smoothing);
                self.histogram[i] = self.histogram[i].clamp(0.05, 0.95);
            }
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
                    KeyCode::Char('n') => app.play_next_track(),
                    KeyCode::Char('p') => app.play_previous_track(),
                    KeyCode::Char('c') => app.toggle_continuous_play(),
                    _ => {}
                }
            }
        }
    }
}
// dividiamo il layout orizzontale in due 40% e 60%
fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(f.area());

    render_file_browser(f, app, chunks[0]);
    render_player_info(f, app, chunks[1]);
}
// parte sinistra relativa alla visone dei file
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
// stabiliamo un layout per la parte sinistra
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

    let track_name = app
        .selected_track_name
        .as_deref()
        .unwrap_or("Nessuna traccia selezionata");
    let title = Paragraph::new(track_name)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(border::ROUNDED)
                .title(" 🎵 Traccia Corrente ")
                .style(Style::default().fg(Color::Green)),
        )
        .style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(title, chunks[0]);

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

    render_volume_control(f, app, chunks[2]);
    render_histogram(f, app, chunks[3]);

    let status = if app.is_playing {
        "▶️  Playing"
    } else if app.selected_track.is_some() {
        "⏸️  Paused"
    } else {
        "⏹️  Stopped"
    };

    let continuous_status = if app.continuous_play {
        " | 🔁 Continua: ON"
    } else {
        " | 🔁 Continua: OFF"
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                status,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                continuous_status,
                Style::default().fg(if app.continuous_play {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            ),
        ]),
        Line::from(""),
        Line::from("Controls: [Space] Play/Pause | [↑↓/jk] Navigate | [Enter] Select"),
        Line::from("          [+/-] Volume | [N] Next | [P] Previous | [C] Continua | [Q] Quit"),
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
        .title(" 📊 Analisi Spettro Audio (FFT Real-Time) ")
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
