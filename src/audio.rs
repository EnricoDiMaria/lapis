use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parakeet_rs::{ExecutionConfig, ParakeetTDT, TimedToken, TimestampMode, Transcriber};
use rodio::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub page: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMemoEntry {
    pub id: String,
    pub filename: String,
    pub duration_ms: u64,
    pub transcript: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LectureSlot {
    pub id: usize,
    pub title: String,
    pub wav: Option<String>,
    pub duration_ms: u64,
    pub page_range: Option<(u32, u32)>,
    pub timeline: Vec<(u64, u32)>,
    pub timeline_is_live: bool,
    pub segments: Vec<TranscriptSegment>,
    pub waveform_peaks: Vec<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioDocIndex {
    pub lecture_title: String,
    pub lecture_wav: Option<String>,
    pub lecture_duration_ms: u64,
    pub page_range: Option<(u32, u32)>,
    pub timeline: Vec<(u64, u32)>,
    pub timeline_is_live: bool,
    pub segments: Vec<TranscriptSegment>,
    pub page_memos: HashMap<u32, Vec<VoiceMemoEntry>>,
    pub waveform_peaks: Vec<f32>,
    pub source_path: Option<PathBuf>,
    pub other_lectures: Vec<LectureSlot>,
    #[serde(default)]
    pub lecture_id: usize,

    #[serde(default = "default_next_lecture_id")]
    pub next_lecture_id: usize,
}

fn default_next_lecture_id() -> usize {
    2
}

impl AudioDocIndex {
    pub fn media_root() -> PathBuf {
        crate::paths::data_dir().join("media")
    }

    pub fn media_dir_for_pdf(pdf_path: &Path) -> PathBuf {
        let dir = Self::media_root().join(crate::docid::resolve_key(pdf_path));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn load(pdf_path: &Path) -> Self {
        if pdf_path.as_os_str().is_empty() {
            return Self::default();
        }

        let index_path = Self::media_dir_for_pdf(pdf_path).join("audio_index.json");

        if let Ok(content) = fs::read_to_string(index_path)
            && let Ok(mut idx) = serde_json::from_str::<Self>(&content)
        {
            idx.ensure_lecture_ids();
            return idx;
        }

        let mut idx = Self::default();
        idx.ensure_lecture_ids();
        idx
    }

    pub fn save(&self, pdf_path: &Path) {
        if pdf_path.as_os_str().is_empty() {
            return;
        }

        let index_path = Self::media_dir_for_pdf(pdf_path).join("audio_index.json");

        let mut to_save = self.clone();

        to_save.ensure_lecture_ids();

        to_save.source_path =
            Some(dunce::canonicalize(pdf_path).unwrap_or_else(|_| pdf_path.to_path_buf()));

        if let Ok(json) = serde_json::to_string_pretty(&to_save) {
            let tmp = index_path.with_extension("json.tmp");

            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, &index_path);
            }
        }
    }

    /// Returns which slide corresponds to a given playback timestamp in milliseconds.
    pub fn page_at_time(&self, pos_ms: u64) -> Option<u32> {
        if self.timeline.is_empty() {
            return None;
        }
        // The timeline is timestamp-sorted; use binary search.
        let idx = self.timeline.partition_point(|&(ts, _)| ts <= pos_ms);
        Some(self.timeline[idx.saturating_sub(1)].1)
    }

    /// Returns the starting timestamp in milliseconds for a given slide, if mapped.
    pub fn time_for_page(&self, page: u32) -> Option<u64> {
        self.timeline
            .iter()
            .find(|&&(_, p)| p == page)
            .map(|&(ts, _)| ts)
    }
    /// Index of the transcript segment playing at `pos_ms` (segments are sorted by start).
    pub fn segment_index_at(&self, pos_ms: u64) -> Option<usize> {
        if self.segments.is_empty() {
            return None;
        }
        let idx = self.segments.partition_point(|s| s.start_ms <= pos_ms);
        Some(idx.saturating_sub(1))
    }

    /// Pagine dei segmenti dai pin/marker già presenti (nessun testo coinvolto).
    pub fn assign_pages_from_timeline(&mut self) {
        let tl = &self.timeline;
        for s in &mut self.segments {
            let idx = tl.partition_point(|&(ts, _)| ts <= s.start_ms);
            s.page = if tl.is_empty() {
                0
            } else {
                tl[idx.saturating_sub(1)].1
            };
        }
    }

    /// Allinea i segmenti già trascritti alle slide `range` (1-based, inclusivo).
    pub fn align_to_slides(&mut self, slide_texts: &[String], range: (u32, u32)) {
        let (lo, hi) = (range.0.max(1), range.1.min(slide_texts.len() as u32));
        if hi < lo || self.segments.is_empty() {
            return;
        }
        let raw: Vec<_> = self
            .segments
            .iter()
            .map(|s| (s.start_ms, s.end_ms, s.text.clone()))
            .collect();
        let total = (self.lecture_duration_ms > 0).then_some(self.lecture_duration_ms);
        let (mut segs, mut tl) =
            align_segments_to_slides(raw, &slide_texts[(lo - 1) as usize..hi as usize], total);
        let off = lo - 1;
        for s in &mut segs {
            s.page += off;
        }
        for (_, p) in &mut tl {
            *p += off;
        }
        self.segments = segs;
        self.timeline = tl;
        self.timeline_is_live = false;
        self.page_range = Some((lo, hi));
    }

    fn take_active(&mut self) -> LectureSlot {
        LectureSlot {
            id: std::mem::replace(&mut self.lecture_id, 0),
            title: std::mem::take(&mut self.lecture_title),
            wav: self.lecture_wav.take(),
            duration_ms: std::mem::take(&mut self.lecture_duration_ms),
            page_range: self.page_range.take(),
            timeline: std::mem::take(&mut self.timeline),
            timeline_is_live: std::mem::take(&mut self.timeline_is_live),
            segments: std::mem::take(&mut self.segments),
            waveform_peaks: std::mem::take(&mut self.waveform_peaks),
        }
    }

    fn put_active(&mut self, s: LectureSlot) {
        self.lecture_id = s.id;
        self.lecture_title = s.title;
        self.lecture_wav = s.wav;
        self.lecture_duration_ms = s.duration_ms;
        self.page_range = s.page_range;
        self.timeline = s.timeline;
        self.timeline_is_live = s.timeline_is_live;
        self.segments = s.segments;
        self.waveform_peaks = s.waveform_peaks;
    }
    pub fn switch_to_other(&mut self, i: usize) {
        self.ensure_lecture_ids();

        if i >= self.other_lectures.len() {
            return;
        }

        let incoming = std::mem::take(&mut self.other_lectures[i]);

        let outgoing = self.take_active();

        self.put_active(incoming);

        self.other_lectures[i] = outgoing;
    }
    pub fn start_new_lecture(&mut self) {
        self.ensure_lecture_ids();

        let cur = self.take_active();

        if cur.wav.is_some() || !cur.segments.is_empty() {
            self.other_lectures.push(cur);
        }

        self.lecture_id = self.allocate_lecture_id();
    }
    pub fn remove_active_lecture(&mut self) {
        self.ensure_lecture_ids();

        let _ = self.take_active();

        if let Some(next) = self.other_lectures.pop() {
            self.put_active(next);
        } else {
            self.lecture_id = self.allocate_lecture_id();
        }
    }
    pub fn fresh_wav_name(&self) -> String {
        let used: HashSet<&str> = self
            .lecture_wav
            .iter()
            .map(String::as_str)
            .chain(self.other_lectures.iter().filter_map(|l| l.wav.as_deref()))
            .collect();
        if !used.contains("lecture.wav") {
            return "lecture.wav".into();
        }
        (2..)
            .map(|n| format!("lecture_{n}.wav"))
            .find(|n| !used.contains(n.as_str()))
            .unwrap()
    }
    pub fn active_covers(&self, page: u32) -> bool {
        self.lecture_wav.is_some() && self.page_range.is_none_or(|(a, b)| (a..=b).contains(&page))
    }
    pub fn other_covering(&self, page: u32) -> Option<usize> {
        if self.active_covers(page) {
            return None;
        }
        self.other_lectures.iter().position(|l| {
            l.wav.is_some() && l.page_range.is_some_and(|(a, b)| (a..=b).contains(&page))
        })
    }
    /// Applica `f` a tutte le pagine di tutte le lezioni (inserimento/eliminazione/riordino pagine).
    pub fn for_each_page_mut(&mut self, mut f: impl FnMut(&mut u32)) {
        fn fix(r: &mut Option<(u32, u32)>, f: &mut impl FnMut(&mut u32)) {
            if let Some((a, b)) = r {
                f(a);
                f(b);
                if *a > *b {
                    std::mem::swap(a, b);
                }
            }
        }
        for (_, p) in &mut self.timeline {
            f(p);
        }
        for s in &mut self.segments {
            f(&mut s.page);
        }
        fix(&mut self.page_range, &mut f);
        for l in &mut self.other_lectures {
            for (_, p) in &mut l.timeline {
                f(p);
            }
            for s in &mut l.segments {
                f(&mut s.page);
            }
            fix(&mut l.page_range, &mut f);
        }
    }

    /// Shifts timeline markers and transcript segments after trimming the lecture audio to `[start_ms, end_ms]`.
    pub fn apply_lecture_trim(&mut self, start_ms: u64, end_ms: u64) {
        let new_dur = end_ms.saturating_sub(start_ms);
        // Preserve the page at the trim start as the zero-time marker.
        let first_page = self.page_at_time(start_ms);
        self.lecture_duration_ms = new_dur;

        self.timeline
            .retain(|&(ts, _)| ts > start_ms && ts < end_ms);
        for (ts, _) in &mut self.timeline {
            *ts -= start_ms;
        }
        if let Some(p) = first_page {
            self.timeline.insert(0, (0, p));
        }
        self.timeline.dedup_by_key(|k| k.1);

        // Filter and shift transcript segments
        self.segments.retain_mut(|seg| {
            if seg.end_ms <= start_ms || seg.start_ms >= end_ms {
                return false;
            }
            seg.start_ms = seg.start_ms.saturating_sub(start_ms);
            seg.end_ms = (seg.end_ms - start_ms).min(new_dur);
            true
        });
    }
    pub fn ensure_lecture_ids(&mut self) {
        if self.lecture_id == 0 {
            self.lecture_id = 1;
        }

        let mut used = HashSet::new();
        used.insert(self.lecture_id);

        for lecture in &self.other_lectures {
            if lecture.id != 0 {
                used.insert(lecture.id);
            }
        }

        let mut candidate = 2usize;

        for lecture in &mut self.other_lectures {
            if lecture.id == 0 {
                while used.contains(&candidate) {
                    candidate += 1;
                }

                lecture.id = candidate;
                used.insert(candidate);
                candidate += 1;
            }
        }

        self.next_lecture_id = used.iter().copied().max().unwrap_or(1) + 1;
    }

    fn allocate_lecture_id(&mut self) -> usize {
        let id = self.next_lecture_id.max(2);
        self.next_lecture_id = id + 1;
        id
    }
}

type SharedWavWriter = Arc<Mutex<Option<hound::WavWriter<std::io::BufWriter<File>>>>>;

#[derive(PartialEq, Clone)]
pub enum RecordingTarget {
    Lecture,
    PageMemo(u32),
}

pub struct AudioEngine {
    // It must be kept alive for as long as audio is playing.
    device_sink: Option<rodio::MixerDeviceSink>,
    player: Option<rodio::Player>,
    pub active_file: Option<PathBuf>,
    playing_label: String,
    pub speed: f32,
    pub follow_pdf: bool,
    // Accurate playback clock tracking across seeks/pauses/speed changes
    base_pos_ms: u64,
    last_resume_instant: Option<Instant>,
    pub total_duration_ms: u64,
    // Live Microphone Recording state
    rec_stream: Option<cpal::Stream>,
    rec_writer: SharedWavWriter,
    rec_raw_path: Option<PathBuf>,
    pub rec_target: Option<RecordingTarget>,
    rec_start_instant: Option<Instant>,
    stretched_active: bool,
    play_speed: f32,
    pending_stretch: Option<(f32, Receiver<Result<PathBuf, String>>)>,
}

impl AudioEngine {
    pub fn new() -> Self {
        let device_sink = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(mut s) => {
                // Avoid rodio printing a message to stderr on drop (breaks TUIs/GUIs)
                s.log_on_drop(false);
                Some(s)
            }
            Err(_) => None,
        };

        Self {
            device_sink,
            player: None,
            active_file: None,
            playing_label: String::new(),
            speed: 1.0,
            follow_pdf: true,
            base_pos_ms: 0,
            last_resume_instant: None,
            total_duration_ms: 0,
            rec_stream: None,
            rec_writer: Arc::new(Mutex::new(None)),
            rec_raw_path: None,
            rec_target: None,
            rec_start_instant: None,
            stretched_active: false,
            play_speed: 1.0,
            pending_stretch: None,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.player
            .as_ref()
            .map(|p| !p.is_paused() && !p.empty())
            .unwrap_or(false)
    }

    pub fn current_pos_ms(&mut self) -> u64 {
        if let Some(player) = &self.player
            && player.empty()
        {
            self.last_resume_instant = None;
            return self.total_duration_ms;
        }
        let elapsed = self
            .last_resume_instant
            .map(|inst| (inst.elapsed().as_millis() as f64 * self.play_speed as f64) as u64)
            .unwrap_or(0);
        (self.base_pos_ms + elapsed).min(self.total_duration_ms.max(1))
    }

    pub fn load_and_play(
        &mut self,
        wav_path: &Path,
        label: String,
        start_ms: u64,
    ) -> Result<(), String> {
        let speed = self.speed;
        let mut stretched = None;
        if (speed - 1.0).abs() >= 0.01 {
            let cached = stretched_cache_path(wav_path, speed);
            if cached.exists() {
                stretched = Some(cached);
            } else {
                self.request_stretch(wav_path, speed);
            }
        }
        self.start_player(wav_path, label, start_ms, stretched)
    }

    pub fn toggle_play_pause(&mut self) {
        let is_paused = match &self.player {
            Some(p) => p.is_paused(),
            None => return,
        };
        if is_paused {
            if let Some(p) = &self.player {
                p.play();
            }
            self.last_resume_instant = Some(Instant::now());
        } else {
            self.base_pos_ms = self.current_pos_ms();
            self.last_resume_instant = None;
            if let Some(p) = &self.player {
                p.pause();
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.rec_stream.is_some()
    }

    pub fn recording_elapsed_ms(&self) -> u64 {
        self.rec_start_instant
            .map(|i| i.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn start_recording(
        &mut self,
        out_raw_wav: PathBuf,
        target: RecordingTarget,
    ) -> Result<(), String> {
        if self.is_recording() {
            return Err("Already recording".to_string());
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No microphone input device found".to_string())?;
        let config = device
            .default_input_config()
            .map_err(|e| format!("Mic config error: {}", e))?;

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: config.sample_rate(),
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let writer = hound::WavWriter::create(&out_raw_wav, spec).map_err(|e| e.to_string())?;
        let shared_writer: SharedWavWriter = Arc::new(Mutex::new(Some(writer)));
        let writer_clone = shared_writer.clone();
        let channels = config.channels() as usize;

        let err_fn = |err| eprintln!("Microphone stream error: {}", err);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_input_stream(
                    config.into(),
                    move |data: &[f32], _: &_| {
                        if let Ok(mut guard) = writer_clone.lock()
                            && let Some(w) = guard.as_mut()
                        {
                            for frame in data.chunks(channels) {
                                let mono: f32 =
                                    frame.iter().copied().sum::<f32>() / (channels as f32);
                                let amp = (mono.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                let _ = w.write_sample(amp);
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())?,
            cpal::SampleFormat::I16 => device
                .build_input_stream(
                    config.into(),
                    move |data: &[i16], _: &_| {
                        if let Ok(mut guard) = writer_clone.lock()
                            && let Some(w) = guard.as_mut()
                        {
                            for frame in data.chunks(channels) {
                                let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                                let _ = w.write_sample((sum / channels as i32) as i16);
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())?,
            _ => return Err("Unsupported microphone sample format".to_string()),
        };

        stream.play().map_err(|e| e.to_string())?;

        self.rec_stream = Some(stream);
        self.rec_writer = shared_writer;
        self.rec_raw_path = Some(out_raw_wav);
        self.rec_target = Some(target);
        self.rec_start_instant = Some(Instant::now());
        Ok(())
    }

    /// Stops recording, finalizes the WAV header and normalizes to 16kHz Mono for ASR.
    pub fn stop_recording(&mut self, final_wav_path: &Path) -> Option<(RecordingTarget, u64)> {
        self.rec_stream.take(); // Drops stream
        if let Ok(mut guard) = self.rec_writer.lock()
            && let Some(writer) = guard.take()
        {
            let _ = writer.finalize();
        }
        let target = self.rec_target.take()?;
        let raw_path = self.rec_raw_path.take()?;
        self.rec_start_instant = None;

        // Convert to 16kHz Mono via ffmpeg if available, otherwise rename raw WAV directly
        if convert_media_to_16k_wav(&raw_path, final_wav_path).is_ok() {
            let _ = fs::remove_file(&raw_path);
        } else {
            let _ = fs::rename(&raw_path, final_wav_path);
        }

        let dur = wav_duration_ms(final_wav_path).unwrap_or(0);
        Some((target, dur))
    }
    fn start_player(
        &mut self,
        original: &Path,
        label: String,
        start_ms: u64,
        stretched: Option<PathBuf>,
    ) -> Result<(), String> {
        let Some(device_sink) = &self.device_sink else {
            return Err("No audio output device available".to_string());
        };
        let is_stretched = stretched.is_some();
        let play_path = stretched.unwrap_or_else(|| original.to_path_buf());
        let file = File::open(&play_path).map_err(|e| e.to_string())?;
        let decoder = rodio::Decoder::try_from(file).map_err(|e| e.to_string())?;

        let play_speed = if is_stretched { self.speed } else { 1.0 };
        let player = rodio::Player::connect_new(device_sink.mixer());
        player.set_speed(1.0);
        player.append(decoder.fade_in(Duration::from_millis(40)));

        if start_ms > 0 {
            let target = if is_stretched {
                (start_ms as f64 / play_speed as f64) as u64
            } else {
                start_ms
            };
            let _ = player.try_seek(Duration::from_millis(target));
        }

        self.player = Some(player);
        self.active_file = Some(original.to_path_buf());
        self.stretched_active = is_stretched;
        self.play_speed = play_speed;
        self.playing_label = label;
        self.total_duration_ms = wav_duration_ms(original).unwrap_or(0);
        self.base_pos_ms = start_ms;
        self.last_resume_instant = Some(Instant::now());
        Ok(())
    }
    fn request_stretch(&mut self, original: &Path, speed: f32) {
        if let Some((s, _)) = &self.pending_stretch
            && (s - speed).abs() < 0.001
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let path = original.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(stretched_wav(&path, speed));
        });
        self.pending_stretch = Some((speed, rx));
    }
    pub fn poll(&mut self) -> bool {
        let msg = match &self.pending_stretch {
            Some((_, rx)) => rx.try_recv(),
            None => return false,
        };
        match msg {
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.pending_stretch = None;
                false
            }
            Ok(res) => {
                let Some((speed, _)) = self.pending_stretch.take() else {
                    return false;
                };
                let Ok(path) = res else {
                    return false;
                };
                if (speed - self.speed).abs() > 0.001 {
                    return false;
                }
                let Some(original) = self.active_file.clone() else {
                    return false;
                };
                let pos = self.current_pos_ms();
                if pos >= self.total_duration_ms {
                    return false;
                }
                let paused = self.player.as_ref().map(|p| p.is_paused()).unwrap_or(false);
                let label = self.playing_label.clone();
                if self.start_player(&original, label, pos, Some(path)).is_ok() {
                    self.apply_pause(paused, pos);
                    return true;
                }
                false
            }
        }
    }
    fn apply_pause(&mut self, paused: bool, pos: u64) {
        if paused {
            if let Some(p) = &self.player {
                p.pause();
            }
            self.base_pos_ms = pos;
            self.last_resume_instant = None;
        }
    }
    pub fn set_speed(&mut self, new_speed: f32) {
        let pos = self.current_pos_ms();
        let paused = self.player.as_ref().map(|p| p.is_paused()).unwrap_or(false);
        let finished = self.total_duration_ms > 0 && pos >= self.total_duration_ms;

        self.speed = new_speed;
        self.base_pos_ms = pos;
        self.last_resume_instant = if paused || finished {
            None
        } else {
            Some(Instant::now())
        };

        if finished {
            return;
        }
        let Some(original) = self.active_file.clone() else {
            return;
        };
        let label = self.playing_label.clone();

        if (new_speed - 1.0).abs() < 0.01 {
            self.pending_stretch = None;
            if self.start_player(&original, label, pos, None).is_ok() {
                self.apply_pause(paused, pos);
            }
            return;
        }

        let cached = stretched_cache_path(&original, new_speed);
        if cached.exists() {
            self.pending_stretch = None;
            if self
                .start_player(&original, label, pos, Some(cached))
                .is_ok()
            {
                self.apply_pause(paused, pos);
            }
        } else {
            self.request_stretch(&original, new_speed);
        }
    }

    pub fn seek_to_ms(&mut self, target_ms: u64) {
        let clamped = target_ms.min(self.total_duration_ms);
        let player_target = if self.stretched_active {
            (clamped as f64 / self.play_speed as f64) as u64
        } else {
            clamped
        };
        if let Some(player) = &self.player
            && player
                .try_seek(Duration::from_millis(player_target))
                .is_ok()
        {
            self.base_pos_ms = clamped;
            if !player.is_paused() {
                self.last_resume_instant = Some(Instant::now());
            }
            return;
        }
        if let Some(path) = self.active_file.clone() {
            let lbl = self.playing_label.clone();
            let _ = self.load_and_play(&path, lbl, clamped);
        }
    }

    pub fn stop_and_release(&mut self) {
        self.player.take();
        self.active_file = None;
        self.last_resume_instant = None;
        self.base_pos_ms = 0;
        self.total_duration_ms = 0;
    }
}
fn stretched_cache_path(input: &Path, speed: f32) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    dunce::canonicalize(input)
        .unwrap_or_else(|_| input.to_path_buf())
        .hash(&mut hasher);
    fs::metadata(input)
        .and_then(|m| m.modified())
        .ok()
        .hash(&mut hasher);
    ((speed * 100.0).round() as u32).hash(&mut hasher);
    std::env::temp_dir()
        .join("lapis_stretch")
        .join(format!("{:016x}.wav", hasher.finish()))
}
/// Converts any audio/video file (.mp4, .mkv, .mp3, .m4a, .wav, .ogg) into 16kHz Mono 16-bit PCM WAV.
pub fn convert_media_to_16k_wav(input: &Path, output_wav: &Path) -> Result<u64, String> {
    let status = crate::sysproc::ffmpeg()
        .args([
            "-y",
            "-i",
            &input.to_string_lossy(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            &output_wav.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    if !status.success() {
        return Err("ffmpeg conversion failed".to_string());
    }
    Ok(wav_duration_ms(output_wav).unwrap_or(0))
}

fn wav_duration_ms(wav_path: &Path) -> Option<u64> {
    let reader = hound::WavReader::open(wav_path).ok()?;
    let spec = reader.spec();
    if spec.sample_rate == 0 || spec.channels == 0 {
        return None;
    }
    let frames = reader.duration() as u64;
    Some((frames * 1000) / (spec.sample_rate as u64))
}

pub fn format_ms(ms: u64) -> String {
    let total_secs = ms / 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

fn atempo_chain(mut s: f32) -> String {
    let mut parts: Vec<String> = Vec::new();
    while s > 2.0 {
        parts.push("atempo=2.0".to_string());
        s /= 2.0;
    }
    while s < 0.5 {
        parts.push("atempo=0.5".to_string());
        s *= 2.0;
    }
    parts.push(format!("atempo={:.4}", s));
    parts.join(",")
}

fn stretched_wav(input: &Path, speed: f32) -> Result<PathBuf, String> {
    let out = stretched_cache_path(input, speed);
    if out.exists() {
        return Ok(out);
    }
    let dir = out.parent().ok_or("bad cache path")?.to_path_buf();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let tmp = out.with_extension("part.wav");
    let status = crate::sysproc::ffmpeg()
        .args(["-y", "-nostdin", "-loglevel", "quiet", "-i"])
        .arg(input)
        .args([
            "-vn",
            "-filter:a",
            &atempo_chain(speed),
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    if !status.success() {
        let _ = fs::remove_file(&tmp);
        return Err("ffmpeg time-stretch failed".to_string());
    }
    fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// ASR: Parakeet TDT 0.6B v3 (ONNX). Model folder, e.g.
// ~/.local/share/lapis/models/parakeet-tdt-0.6b-v3-int8/ with vocab.txt,
// encoder-model(.int8).onnx and decoder_joint-model(.int8).onnx.
// Override with LAPIS_ASR_MODEL; thread count with LAPIS_ASR_THREADS.
// ---------------------------------------------------------------------------

const ASR_SR: usize = 16_000;
/// Max audio per model call (the model only handles a few minutes at once).
const CHUNK_SECS: usize = 40;
const MIN_SILENCE_MS: u64 = 1500;
const SILENCE_PAD_MS: u64 = 300;

/// Messages sent by background transcription jobs.
pub enum AsrJobResult {
    /// Aligned transcript so far; `finished` marks the last message of a job.
    Lecture {
        segments: Vec<TranscriptSegment>,
        done_ms: u64,
        total_ms: u64,
        finished: bool,
    },
    Aligned {
        segments: Vec<TranscriptSegment>,
        timeline: Vec<(u64, u32)>,
        range: (u32, u32),
    },
    Memo {
        page: u32,
        memo_idx: usize,
        text: String,
    },
    Cancelled,
    Error(String),
}

enum AsrStop {
    Cancelled,
    Failed(String),
}

impl From<String> for AsrStop {
    fn from(e: String) -> Self {
        AsrStop::Failed(e)
    }
}

impl AsrStop {
    fn into_job(self) -> AsrJobResult {
        match self {
            AsrStop::Cancelled => AsrJobResult::Cancelled,
            AsrStop::Failed(e) => AsrJobResult::Error(e),
        }
    }
}

pub fn is_asr_model_dir(p: &Path) -> bool {
    p.join("vocab.txt").is_file()
        && fs::read_dir(p)
            .map(|rd| {
                rd.flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    n.starts_with("encoder") && n.ends_with(".onnx")
                })
            })
            .unwrap_or(false)
}

pub fn find_asr_model() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LAPIS_ASR_MODEL").map(PathBuf::from)
        && is_asr_model_dir(&p)
    {
        return Some(p);
    }

    let models_dir = crate::paths::models_dir();

    for name in [
        "parakeet-tdt-0.6b-v3-int8",
        "parakeet-tdt-0.6b-v3",
        "parakeet-ultra",
    ] {
        let p = models_dir.join(name);
        if is_asr_model_dir(&p) {
            return Some(p);
        }
    }

    fs::read_dir(&models_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && is_asr_model_dir(p))
}

type SharedAsr = Arc<Mutex<ParakeetTDT>>;

// The model is loaded once and kept in memory. Its inner mutex is taken per
// chunk, so short jobs (voice memos) never wait for a whole lecture.
static ASR_MODEL: OnceLock<Mutex<Option<(PathBuf, SharedAsr)>>> = OnceLock::new();
static ASR_EXEC: OnceLock<ExecutionConfig> = OnceLock::new();

fn asr_exec_config() -> Result<ExecutionConfig, String> {
    if let Some(c) = ASR_EXEC.get() {
        return Ok(c.clone());
    }
    // ONNX Runtime allows a single environment per process
    let env = parakeet_rs::ort::init()
        .build()
        .map_err(|e| format!("ONNX Runtime init failed: {}", e))?;
    let threads = std::env::var("LAPIS_ASR_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().clamp(2, 8))
                .unwrap_or(4)
        });
    let cfg = ExecutionConfig::new()
        .with_environment(env)
        .with_intra_threads(threads);
    let _ = ASR_EXEC.set(cfg.clone());
    Ok(cfg)
}

fn get_asr(model_dir: &Path) -> Result<SharedAsr, String> {
    let cell = ASR_MODEL.get_or_init(|| Mutex::new(None));
    let mut guard = cell
        .lock()
        .map_err(|_| "ASR cache lock poisoned".to_string())?;

    if let Some((cached, m)) = guard.as_ref()
        && cached == model_dir
    {
        return Ok(m.clone());
    }

    let model = ParakeetTDT::from_pretrained(model_dir, Some(asr_exec_config()?))
        .map_err(|e| format!("Parakeet init failed: {}", e))?;
    let shared: SharedAsr = Arc::new(Mutex::new(model));
    *guard = Some((model_dir.to_path_buf(), shared.clone()));
    Ok(shared)
}

/// Loads the model ahead of time (call from a background thread).
pub fn preload_asr_model(model_dir: &Path) {
    let _ = get_asr(model_dir);
}

/// Speech intervals `(start_ms, end_ms)`; silences >= `min_silence_ms` are dropped,
/// keeping `pad_ms` of margin around speech.
fn speech_intervals(samples: &[f32], min_silence_ms: u64, pad_ms: u64) -> Vec<(u64, u64)> {
    const FRAME_MS: usize = 50;
    let frame_len = ASR_SR * FRAME_MS / 1000;
    let n = samples.len() / frame_len;
    if n == 0 {
        return Vec::new();
    }

    let rms: Vec<f32> = samples
        .chunks_exact(frame_len)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .collect();

    // Adaptive threshold: ~3x the noise floor (10th percentile)
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let thr = (sorted[sorted.len() / 10] * 3.0).clamp(0.004, 0.02);

    let mut voiced: Vec<bool> = rms.iter().map(|&r| r >= thr).collect();

    // Short pauses stay untouched
    let min_sil = ((min_silence_ms as usize) / FRAME_MS).max(1);
    let mut i = 0;
    while i < n {
        if voiced[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !voiced[i] {
            i += 1;
        }
        if i - start < min_sil {
            voiced[start..i].fill(true);
        }
    }

    let total_ms = (samples.len() as u64 * 1000) / ASR_SR as u64;
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut i = 0;
    while i < n {
        if !voiced[i] {
            i += 1;
            continue;
        }
        let s = i;
        while i < n && voiced[i] {
            i += 1;
        }
        let a = ((s * FRAME_MS) as u64).saturating_sub(pad_ms);
        let b = ((i * FRAME_MS) as u64 + pad_ms).min(total_ms);
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Concatenates speech intervals. Returns the samples and a map `(old_start, old_end, new_start)` in ms.
fn compact_speech(samples: &[f32], intervals: &[(u64, u64)]) -> (Vec<f32>, Vec<(u64, u64, u64)>) {
    let per_ms = ASR_SR / 1000;
    let mut out = Vec::with_capacity(samples.len());
    let mut map = Vec::with_capacity(intervals.len());
    for &(a, b) in intervals {
        let s = (a as usize * per_ms).min(samples.len());
        let e = (b as usize * per_ms).min(samples.len());
        if e <= s {
            continue;
        }
        map.push((a, b, (out.len() / per_ms) as u64));
        out.extend_from_slice(&samples[s..e]);
    }
    (out, map)
}

/// Maps a timestamp of the compacted audio back to the original timeline.
fn remap_to_original(t_new: u64, map: &[(u64, u64, u64)], is_end: bool) -> u64 {
    let idx = if is_end {
        map.partition_point(|&(_, _, ns)| ns < t_new)
    } else {
        map.partition_point(|&(_, _, ns)| ns <= t_new)
    };
    if idx == 0 {
        return map.first().map(|m| m.0).unwrap_or(t_new);
    }
    let (os, oe, ns) = map[idx - 1];
    (os + (t_new - ns)).min(oe)
}

/// Splits audio into chunks of at most `max_len` samples, cutting at the quietest
/// point of the last 30% of each chunk.
fn plan_chunks(samples: &[f32], max_len: usize) -> Vec<(usize, usize)> {
    let frame = (ASR_SR / 100).max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < samples.len() {
        if samples.len() - start <= max_len {
            out.push((start, samples.len()));
            break;
        }
        let (lo, hi) = (start + max_len * 7 / 10, start + max_len);
        let mut best = hi;
        let mut best_e = f32::MAX;
        let mut i = lo;
        while i + frame <= hi {
            let e: f32 = samples[i..i + frame].iter().map(|s| s * s).sum();
            if e < best_e {
                best_e = e;
                best = i + frame / 2;
            }
            i += frame;
        }
        out.push((start, best));
        start = best;
    }
    out
}

fn is_closing_punct(w: &str) -> bool {
    !w.is_empty()
        && w.chars()
            .all(|c| matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | ')' | '…' | '%'))
}

fn ends_sentence(w: &str) -> bool {
    w.ends_with(['.', '?', '!', '…'])
}

/// Groups timed words (seconds, chunk-relative) into sentence segments: closed at sentence
/// end, after a pause >= 1s, or after ~12s. Returns chunk-relative `(start_ms, end_ms, text)`.
fn words_to_segments(words: &[TimedToken]) -> Vec<(u64, u64, String)> {
    let to_ms = |s: f32| (s.max(0.0) * 1000.0).round() as u64;
    let mut out = Vec::new();
    let mut text = String::new();
    let (mut seg_start, mut seg_end) = (0.0f32, 0.0f32);

    for (i, w) in words.iter().enumerate() {
        let wt = w.text.trim();
        if wt.is_empty() {
            continue;
        }
        if text.is_empty() {
            seg_start = w.start;
        } else if !is_closing_punct(wt) {
            text.push(' ');
        }
        text.push_str(wt);
        seg_end = w.end.max(seg_end);

        let gap_next = words.get(i + 1).map(|n| n.start - w.end).unwrap_or(0.0);
        if ends_sentence(wt)
            || gap_next >= 1.0
            || seg_end - seg_start >= 12.0
            || i + 1 == words.len()
        {
            let t = text.trim().to_string();
            if !t.is_empty() {
                out.push((to_ms(seg_start), to_ms(seg_end).max(to_ms(seg_start)), t));
            }
            text.clear();
        }
    }
    out
}

fn transcribe_chunk(model: &SharedAsr, chunk: &[f32]) -> Result<Vec<TimedToken>, String> {
    // The model needs at least ~1s of input
    let mut audio = chunk.to_vec();
    if audio.len() < ASR_SR {
        audio.resize(ASR_SR, 0.0);
    }
    let mut m = model
        .lock()
        .map_err(|_| "ASR model lock poisoned".to_string())?;
    m.transcribe_samples(audio, ASR_SR as u32, 1, Some(TimestampMode::Words))
        .map(|r| r.tokens)
        .map_err(|e| format!("Parakeet inference error: {}", e))
}

/// Chunked transcription on the original timeline. `on_chunk(all_segments, done_ms, total_ms)`
/// runs after each chunk. Returns all segments and the total audio duration.
fn transcribe_chunks(
    model_dir: &Path,
    wav_path: &Path,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(&[(u64, u64, String)], u64, u64),
) -> Result<(Vec<(u64, u64, String)>, u64), AsrStop> {
    let samples = load_wav_samples_f32(wav_path)?;
    if samples.is_empty() {
        return Err(AsrStop::Failed("Audio file is empty".to_string()));
    }
    let total_ms = (samples.len() as u64 * 1000) / ASR_SR as u64;

    // Skip long silences when it saves at least 8%
    let (samples, remap) = if samples.len() >= ASR_SR * 10 {
        let iv = speech_intervals(&samples, MIN_SILENCE_MS, SILENCE_PAD_MS);
        let kept_ms: u64 = iv.iter().map(|&(a, b)| b - a).sum();
        if !iv.is_empty() && kept_ms * 100 <= total_ms * 92 {
            let (c, m) = compact_speech(&samples, &iv);
            (c, Some(m))
        } else {
            (samples, None)
        }
    } else {
        (samples, None)
    };
    let map_t = |t: u64, is_end: bool| match &remap {
        Some(m) => remap_to_original(t, m, is_end),
        None => t,
    };

    let model = get_asr(model_dir)?;
    let chunks = plan_chunks(&samples, CHUNK_SECS * ASR_SR);

    let mut all: Vec<(u64, u64, String)> = Vec::new();
    for &(a, b) in &chunks {
        if cancel.load(Ordering::Relaxed) {
            return Err(AsrStop::Cancelled);
        }

        let words = transcribe_chunk(&model, &samples[a..b])?;
        let chunk_off_ms = (a as u64 * 1000) / ASR_SR as u64;
        let mut floor = all.last().map(|s| s.1).unwrap_or(0);

        for (s0, s1, text) in words_to_segments(&words) {
            let t0 = map_t(chunk_off_ms + s0, false).max(floor);
            let t1 = map_t(chunk_off_ms + s1, true).max(t0);
            floor = t1;
            all.push((t0, t1, text));
        }

        let done_ms = map_t((b as u64 * 1000) / ASR_SR as u64, true);
        on_chunk(&all, done_ms, total_ms);
    }

    Ok((all, total_ms))
}

/// Transcribes the lecture and aligns it to slides, emitting partial results after each chunk.
/// `live_timeline`: recorded/pinned slide markers to trust instead of text alignment.
pub fn transcribe_lecture(
    model_dir: &Path,
    wav_path: &Path,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(AsrJobResult),
) {
    let to_segs = |v: &[(u64, u64, String)]| -> Vec<TranscriptSegment> {
        v.iter()
            .map(|(a, b, t)| TranscriptSegment {
                start_ms: *a,
                end_ms: *b,
                page: 0,
                text: t.clone(),
            })
            .collect()
    };
    let res = transcribe_chunks(
        model_dir,
        wav_path,
        cancel,
        &mut |segs, done_ms, total_ms| {
            emit(AsrJobResult::Lecture {
                segments: to_segs(segs),
                done_ms,
                total_ms,
                finished: false,
            });
        },
    );
    emit(match res {
        Ok((segs, total_ms)) => AsrJobResult::Lecture {
            segments: to_segs(&segs),
            done_ms: total_ms,
            total_ms,
            finished: true,
        },
        Err(e) => e.into_job(),
    });
}

/// Transcribes a voice memo into plain text.
pub fn transcribe_memo(
    model_dir: &Path,
    wav_path: &Path,
    page: u32,
    memo_idx: usize,
) -> AsrJobResult {
    let never = AtomicBool::new(false);
    match transcribe_chunks(model_dir, wav_path, &never, &mut |_, _, _| {}) {
        Ok((segs, _)) => AsrJobResult::Memo {
            page,
            memo_idx,
            text: segs.into_iter().map(|s| s.2).collect::<Vec<_>>().join(" "),
        },
        Err(e) => e.into_job(),
    }
}

fn load_wav_samples_f32(wav_path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let channels = (spec.channels as usize).max(1);

    let raw_samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .filter_map(Result::ok)
            .map(|s| (s as f32) / (i16::MAX as f32))
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
    };

    let mono: Vec<f32> = if channels == 1 {
        raw_samples
    } else {
        raw_samples
            .chunks(channels)
            .map(|c| c.iter().copied().sum::<f32>() / (channels as f32))
            .collect()
    };

    if spec.sample_rate == 16000 || spec.sample_rate == 0 || mono.is_empty() {
        return Ok(mono);
    }

    // Linear-interpolation resampling to 16 kHz
    let ratio = spec.sample_rate as f64 / 16000.0;
    let out_len = (mono.len() as f64 / ratio) as usize;
    let last = mono.len() - 1;
    let mut resampled = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let a = mono[idx.min(last)];
        let b = mono[(idx + 1).min(last)];
        resampled.push(a + (b - a) * frac);
    }
    Ok(resampled)
}

/// Very common words that don't help tell slides apart.
const STOPWORDS: &[&str] = &[
    "sono", "della", "delle", "dello", "degli", "anche", "questo", "questa", "questi", "queste",
    "quello", "quella", "come", "dove", "quando", "perché", "perche", "allora", "quindi", "cioè",
    "cioe", "ecco", "dunque", "però", "pero", "molto", "tutti", "tutto", "tutta", "ogni", "nella",
    "nelle", "sulla", "sulle", "dalla", "alla", "alle", "that", "this", "with", "from", "have",
    "will", "what", "there", "their", "which", "about", "che", "per", "con", "non", "una", "gli",
    "the", "and", "for",
];

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' => 'a',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ô' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        c => c,
    }
}

/// Words reduced to a 6-char stem so inflections match ("algoritmo"/"algoritmi").
fn extract_stems(text: &str) -> HashSet<String> {
    let cleaned: String = text
        .to_lowercase()
        .chars()
        .map(fold)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();

    cleaned
        .split_whitespace()
        .filter(|w| {
            let n = w.chars().count();
            (n >= 4
                || (n >= 3 && w.chars().all(|c| c.is_ascii_alphanumeric()))
                || (n >= 2 && w.chars().any(|c| c.is_ascii_digit())))
                && !STOPWORDS.contains(w)
        })
        .map(|w| w.chars().take(6).collect::<String>())
        .collect()
}

/// Weight of text similarity against the time prior and jump penalties.
const SIM_WEIGHT: f32 = 1.75;

fn despike(pages: &mut [u32], min_run: usize) {
    let mut i = 0;
    while i < pages.len() {
        let mut j = i;
        while j < pages.len() && pages[j] == pages[i] {
            j += 1;
        }
        if j - i < min_run && i > 0 && j < pages.len() && pages[i - 1] == pages[j] {
            let v = pages[i - 1];
            for x in &mut pages[i..j] {
                *x = v;
            }
        }
        i = j;
    }
}

/// Viterbi alignment of transcript segments to slides using IDF-weighted stem overlap.
/// `total_ms` (full audio length) anchors the time prior when the transcript is partial.
pub fn align_segments_to_slides(
    raw_segments: Vec<(u64, u64, String)>,
    slide_texts: &[String],
    total_ms: Option<u64>,
) -> (Vec<TranscriptSegment>, Vec<(u64, u32)>) {
    let n_slides = slide_texts.len().max(1);
    let k_segs = raw_segments.len();
    if k_segs == 0 {
        return (Vec::new(), Vec::new());
    }

    let slide_features: Vec<HashSet<String>> =
        slide_texts.iter().map(|s| extract_stems(s)).collect();

    // Inverse index stem -> slides, and IDF (a stem on every slide weighs 0)
    let mut inv: HashMap<&str, Vec<usize>> = HashMap::new();
    for (p, feats) in slide_features.iter().enumerate() {
        for t in feats {
            inv.entry(t.as_str()).or_default().push(p);
        }
    }
    let idf: HashMap<&str, f32> = inv
        .iter()
        .map(|(t, v)| {
            let w = ((n_slides as f32 + 1.0) / (v.len() as f32 + 1.0))
                .ln()
                .max(0.0);
            (*t, w)
        })
        .collect();

    // Transition penalties, indexed by (p - q) + n_slides - 1
    let pen: Vec<f32> = (0..(2 * n_slides - 1))
        .map(|i| match (i as isize) - (n_slides as isize - 1) {
            0 => 0.0,
            1 => 0.18,
            2 => 0.55,
            -1 => 0.70,
            d if d > 2 => 0.8 + (d as f32) * 0.65,
            d => 1.4 + (d.abs() as f32) * 0.9,
        })
        .collect();

    let last_t = total_ms
        .or_else(|| raw_segments.last().map(|s| s.0))
        .unwrap_or(1)
        .max(1) as f32;

    let mut raw_sim = vec![0.0f32; k_segs * n_slides];
    for (k, (_, _, text)) in raw_segments.iter().enumerate() {
        let feats = extract_stems(text);
        let norm = (feats.len() as f32).sqrt().max(1.0);
        for t in &feats {
            if let Some(list) = inv.get(t.as_str()) {
                let w = idf.get(t.as_str()).copied().unwrap_or(0.0);
                for &p in list {
                    raw_sim[k * n_slides + p] += w / norm;
                }
            }
        }
    }
    const W: isize = 3;
    let mut sim = vec![0.0f32; k_segs * n_slides];
    for k in 0..k_segs {
        let mut wsum = 0.0;
        for d in -W..=W {
            let j = k as isize + d;
            if j < 0 || j >= k_segs as isize {
                continue;
            }
            let w = 1.0 - (d.abs() as f32) / (W as f32 + 1.0);
            wsum += w;
            for p in 0..n_slides {
                sim[k * n_slides + p] += w * raw_sim[j as usize * n_slides + p];
            }
        }
        for p in 0..n_slides {
            sim[k * n_slides + p] /= wsum;
        }
    }

    let mut prev = vec![0.0f32; n_slides];
    let mut cur = vec![f32::NEG_INFINITY; n_slides];
    let mut back = vec![0usize; k_segs * n_slides];

    for (k, (t0, _t1, _text)) in raw_segments.iter().enumerate() {
        let time_frac = (*t0 as f32) / last_t;

        for p in 0..n_slides {
            let sim_kp = SIM_WEIGHT * sim[k * n_slides + p];
            let slide_frac = (p as f32) / ((n_slides.max(2) - 1) as f32);
            let emission = sim_kp - (time_frac - slide_frac).abs() * 0.35;

            if k == 0 {
                cur[p] = emission - (p as f32) * 0.4;
            } else {
                let base = p + n_slides - 1;
                let (mut best, mut best_q) = (f32::NEG_INFINITY, 0);
                for q in 0..n_slides {
                    let cand = prev[q] - pen[base - q];
                    if cand > best {
                        best = cand;
                        best_q = q;
                    }
                }
                cur[p] = best + emission;
                back[k * n_slides + p] = best_q;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }

    // `prev` holds the last computed row
    let mut cur_p = (0..n_slides)
        .max_by(|&a, &b| {
            prev[a]
                .partial_cmp(&prev[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    let mut assigned_pages = vec![1u32; k_segs];
    for k in (0..k_segs).rev() {
        assigned_pages[k] = (cur_p + 1) as u32;
        cur_p = back[k * n_slides + cur_p];
    }
    despike(&mut assigned_pages, 3);

    let mut segments = Vec::with_capacity(k_segs);
    let mut timeline = Vec::new();
    let mut last_timeline_page = 0u32;

    for ((t0, t1, text), page) in raw_segments.into_iter().zip(assigned_pages) {
        if page != last_timeline_page {
            timeline.push((t0, page));
            last_timeline_page = page;
        }
        segments.push(TranscriptSegment {
            start_ms: t0,
            end_ms: t1,
            page,
            text,
        });
    }

    (segments, timeline)
}

/// Trims a WAV file in-place to the interval `[start_ms, end_ms]` and returns the new duration in ms.
pub fn trim_wav_file(wav_path: &Path, start_ms: u64, end_ms: u64) -> Result<u64, String> {
    if end_ms <= start_ms {
        return Err("End time must be greater than start time".to_string());
    }

    let tmp_path = wav_path.with_extension("trim_tmp.wav");
    {
        let mut reader = hound::WavReader::open(wav_path).map_err(|e| e.to_string())?;
        let spec = reader.spec();
        let sr = spec.sample_rate as u64;
        let ch = (spec.channels as u64).max(1);

        // Seek directly to the first frame instead of decoding the preamble.
        let start_frame = (start_ms * sr) / 1000;
        let n_samples = ((((end_ms - start_ms) * sr) / 1000) * ch) as usize;
        reader.seek(start_frame as u32).map_err(|e| e.to_string())?;

        let mut writer = hound::WavWriter::create(&tmp_path, spec).map_err(|e| e.to_string())?;

        match spec.sample_format {
            hound::SampleFormat::Int => {
                for s in reader.samples::<i16>().take(n_samples).flatten() {
                    let _ = writer.write_sample(s);
                }
            }
            hound::SampleFormat::Float => {
                for s in reader.samples::<f32>().take(n_samples).flatten() {
                    let _ = writer.write_sample(s);
                }
            }
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }

    fs::rename(&tmp_path, wav_path).map_err(|e| e.to_string())?;
    Ok(wav_duration_ms(wav_path).unwrap_or(end_ms - start_ms))
}
/// Computes `num_bins` (e.g. 240) normalized amplitude peaks in `[0.0, 1.0]` for fast UI waveform rendering.
pub fn compute_waveform_peaks(wav_path: &Path, num_bins: usize) -> Vec<f32> {
    let Ok(mut reader) = hound::WavReader::open(wav_path) else {
        return Vec::new();
    };
    let total_samples = reader.len() as usize;
    if total_samples == 0 || num_bins == 0 {
        return Vec::new();
    }

    let samples_per_bin = (total_samples / num_bins).max(1);
    let mut peaks = Vec::with_capacity(num_bins);
    let mut current_max = 0.0_f32;
    let mut count = 0usize;

    match reader.spec().sample_format {
        hound::SampleFormat::Int => {
            for s in reader.samples::<i16>().flatten() {
                let amp = (s as f32 / i16::MAX as f32).abs();
                if amp > current_max {
                    current_max = amp;
                }
                count += 1;
                if count >= samples_per_bin {
                    peaks.push(current_max.clamp(0.02, 1.0));
                    current_max = 0.0;
                    count = 0;
                    if peaks.len() >= num_bins {
                        break;
                    }
                }
            }
        }
        hound::SampleFormat::Float => {
            for s in reader.samples::<f32>().flatten() {
                let amp = s.abs();
                if amp > current_max {
                    current_max = amp;
                }
                count += 1;
                if count >= samples_per_bin {
                    peaks.push(current_max.clamp(0.02, 1.0));
                    current_max = 0.0;
                    count = 0;
                    if peaks.len() >= num_bins {
                        break;
                    }
                }
            }
        }
    }

    if count > 0 && peaks.len() < num_bins {
        peaks.push(current_max.clamp(0.02, 1.0));
    }

    // Normalize relative to the maximum peak so quiet recordings still show a clear waveform
    let global_max = peaks.iter().copied().fold(0.05_f32, f32::max);
    for p in &mut peaks {
        *p = (*p / global_max).clamp(0.04, 1.0);
    }
    peaks
}

/// Automatically strips dead-air silences (>= `min_silence_ms`, e.g. 1800ms) from the lecture WAV
/// and remaps slide markers and transcript timestamps onto the compacted audio.
pub fn strip_silences_from_lecture(
    wav_path: &Path,
    index: &mut AudioDocIndex,
    min_silence_ms: u64,
) -> Result<(u64, u64), String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let sr = (spec.sample_rate as usize).max(1);
    let ch = (spec.channels as usize).max(1);

    let all_samples: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Int => reader.samples::<i16>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .filter_map(Result::ok)
            .map(|f| (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect(),
    };

    let old_dur_ms = ((all_samples.len() / ch) as u64 * 1000) / (sr as u64);
    if old_dur_ms < 3000 {
        return Err("Recording is too short to strip silences".to_string());
    }

    // Analyze in 100ms chunks
    let chunk_ms = 100u64;
    let samples_per_chunk = (sr * ch) / 10;
    if samples_per_chunk == 0 {
        return Err("Invalid sample rate".to_string());
    }

    let rms_threshold = 380.0_f32; // ~ -38 dBFS silence threshold for 16-bit PCM
    let num_chunks = all_samples.len() / samples_per_chunk;
    let mut is_voice = vec![false; num_chunks];

    for (c_idx, chunk) in all_samples.chunks_exact(samples_per_chunk).enumerate() {
        let sum_sq: f64 = chunk.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum_sq / chunk.len() as f64).sqrt() as f32;
        is_voice[c_idx] = rms >= rms_threshold;
    }

    // Bridge short pauses (< min_silence_ms) and add 200ms natural breath padding around speech
    let min_silent_chunks = (min_silence_ms / chunk_ms).max(4) as usize;
    let mut i = 0;
    while i < num_chunks {
        if !is_voice[i] {
            let start = i;
            while i < num_chunks && !is_voice[i] {
                i += 1;
            }
            let silent_len = i - start;
            if silent_len < min_silent_chunks {
                // Keep short natural pauses intact!
                for flag in &mut is_voice[start..i] {
                    *flag = true;
                }
            } else {
                // Keep 200ms padding at the edges of long silences
                if start < num_chunks {
                    is_voice[start] = true;
                }
                if start + 1 < num_chunks {
                    is_voice[start + 1] = true;
                }
                if i >= 2 {
                    is_voice[i - 1] = true;
                    is_voice[i - 2] = true;
                }
            }
        } else {
            i += 1;
        }
    }

    // Build contiguous kept intervals `(old_start_ms, old_end_ms, new_start_ms)`
    let mut kept_intervals: Vec<(u64, u64, u64)> = Vec::new();
    let mut new_accum_ms = 0u64;
    let mut c = 0;
    while c < num_chunks {
        if is_voice[c] {
            let s_chunk = c;
            while c < num_chunks && is_voice[c] {
                c += 1;
            }
            let old_s = (s_chunk as u64) * chunk_ms;
            let old_e = (c as u64) * chunk_ms;
            let len_ms = old_e - old_s;
            kept_intervals.push((old_s, old_e, new_accum_ms));
            new_accum_ms += len_ms;
        } else {
            c += 1;
        }
    }

    if kept_intervals.is_empty() || new_accum_ms >= old_dur_ms.saturating_sub(500) {
        return Err("No long silences detected in this track".to_string());
    }

    // Write compacted WAV file
    let tmp_path = wav_path.with_extension("vad_tmp.wav");
    {
        let mut writer = hound::WavWriter::create(&tmp_path, spec).map_err(|e| e.to_string())?;
        for &(old_s, old_e, _) in &kept_intervals {
            let s_idx = ((old_s as usize) * sr * ch) / 1000;
            let e_idx = (((old_e as usize) * sr * ch) / 1000).min(all_samples.len());
            for &sample in &all_samples[s_idx..e_idx] {
                let _ = writer.write_sample(sample);
            }
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp_path, wav_path).map_err(|e| e.to_string())?;

    // Helper closure to remap any old timestamp `t_ms` onto the compacted timeline
    let remap_ts = |t_ms: u64| -> u64 {
        for &(old_s, old_e, new_s) in &kept_intervals {
            if t_ms <= old_s {
                return new_s;
            }
            if t_ms <= old_e {
                return new_s + (t_ms - old_s);
            }
        }
        new_accum_ms
    };

    for (ts, _) in &mut index.timeline {
        *ts = remap_ts(*ts);
    }
    for seg in &mut index.segments {
        seg.start_ms = remap_ts(seg.start_ms);
        seg.end_ms = remap_ts(seg.end_ms).max(seg.start_ms + 100);
    }

    index.lecture_duration_ms = new_accum_ms;
    index.waveform_peaks = compute_waveform_peaks(wav_path, 240);

    Ok((old_dur_ms, new_accum_ms))
}
