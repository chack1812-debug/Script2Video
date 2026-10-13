use std::path::{Path, PathBuf};

use s2v_core::{BgmConfig, EventType, TimelineEvent};
use tracing::{info, warn};

/// XML属性値へ埋め込む文字列をエスケープする(`&`, `<`, `>`, `"`)。
/// `&`は他の置換で生成される実体参照とは無関係なので最初に処理する。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// FCPXMLの`format`リソースが宣言するフレームレート。
/// タイムラインの実時間（tick数, 30000分の1秒単位）自体はどちらでも共通のため、
/// SRT/WAVとの時間基準のズレは生じない。ここで変わるのは NLE 側の
/// フレームスナップ・表示上のフレームレート宣言のみ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameRate {
    /// 字幕ズレ調査(Obsidian記録)で確定した、SRT/WAVと整合する既定値。
    #[default]
    Fps30,
    /// NTSC ドロップフレーム相当(29.97fps)。この経路が必要な場合のみ明示的に指定する。
    Fps2997,
}

impl FrameRate {
    fn format_name(self) -> &'static str {
        match self {
            FrameRate::Fps30 => "FFVideoFormat1080p30",
            FrameRate::Fps2997 => "FFVideoFormat1080p2997",
        }
    }

    /// タイムベースは両者とも30000分の1秒で共通。1フレームぶんのtick数のみ異なる。
    fn frame_duration_attr(self) -> &'static str {
        match self {
            FrameRate::Fps30 => "1000/30000s",
            FrameRate::Fps2997 => "1001/30000s",
        }
    }
}

pub struct Exporter<'a> {
    events: &'a [TimelineEvent],
    output_dir: PathBuf,
    sample_rate: u32,
    bgm_config: BgmConfig,
    fcpxml_fps: FrameRate,
}

impl<'a> Exporter<'a> {
    pub fn new(
        events: &'a [TimelineEvent],
        output_dir: impl Into<PathBuf>,
        sample_rate: u32,
        bgm_config: BgmConfig,
    ) -> Self {
        Self {
            events,
            output_dir: output_dir.into(),
            sample_rate,
            bgm_config,
            fcpxml_fps: FrameRate::default(),
        }
    }

    /// FCPXMLの`format`が宣言するフレームレートを変更する（既定は30fps）。
    pub fn with_fcpxml_fps(mut self, fps: FrameRate) -> Self {
        self.fcpxml_fps = fps;
        self
    }

    pub fn generate_srt(&self, suffix: &str) -> anyhow::Result<()> {
        let dir = self.output_dir.join("timeline");
        std::fs::create_dir_all(&dir)?;
        let path = with_suffix(&dir.join("subtitles.srt"), suffix);

        let mut subtitle_events: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.event_type == EventType::Audio || e.event_type == EventType::Paragraph)
            .collect();
        subtitle_events.sort_by(|a, b| a.start_ms.partial_cmp(&b.start_ms).unwrap());

        let mut content = String::new();
        for (i, event) in subtitle_events.iter().enumerate() {
            let start_s = event.start_ms / 1000.0;
            let end_s = match event.event_type {
                EventType::Paragraph => start_s,
                _ => (event.start_ms + event.duration_ms) / 1000.0,
            };
            content.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                i + 1,
                format_srt_time(start_s),
                format_srt_time(end_s),
                event.display_text.as_deref().unwrap_or(""),
            ));
        }

        std::fs::write(&path, &content)?;
        info!("SRT exported to: {}", path.display());
        Ok(())
    }

    /// タイムラインを機械可読な JSON として書き出す（動画合成の正式入力）。
    pub fn generate_timeline_json(&self, suffix: &str) -> anyhow::Result<()> {
        let dir = self.output_dir.join("timeline");
        std::fs::create_dir_all(&dir)?;
        let path = with_suffix(&dir.join("timeline.json"), suffix);

        let total_ms = self
            .events
            .iter()
            .map(|e| e.start_ms + e.duration_ms)
            .fold(0.0_f64, f64::max);

        let events: Vec<JsonEvent> = self
            .events
            .iter()
            .map(|e| JsonEvent {
                event_type: e.event_type.clone(),
                start_ms: e.start_ms,
                duration_ms: e.duration_ms,
                path: e
                    .path
                    .as_ref()
                    .map(|p| rel_path_string(p, &self.output_dir)),
                name: e.name.clone(),
                text: e.text.clone(),
                display_text: e.display_text.clone(),
                cast: e.cast.clone(),
            })
            .collect();

        let doc = serde_json::json!({
            "version": 1,
            "sample_rate": self.sample_rate,
            "total_ms": total_ms,
            "events": events,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
        info!("timeline.json exported to: {}", path.display());
        Ok(())
    }

    pub fn generate_fcpxml(&self, suffix: &str) -> anyhow::Result<()> {
        let dir = self.output_dir.join("timeline");
        std::fs::create_dir_all(&dir)?;
        let path = with_suffix(&dir.join("timeline.fcpxml"), suffix);

        // タイムラインの総長さ = max(audio_end, bgm_end, se_end) (Python版 exporter.py:33-47 相当)
        let audio_end = self
            .events
            .iter()
            .map(|e| (e.start_ms + e.duration_ms) / 1000.0)
            .fold(0.0_f64, f64::max);
        let bgm_end = self
            .compute_bgm_segments()
            .iter()
            .map(|seg| seg.mix_start_s + seg.mix_duration_s)
            .fold(0.0_f64, f64::max);
        let se_end = self
            .events
            .iter()
            .filter(|e| e.event_type == EventType::Se)
            .filter_map(|e| {
                let p = e.path.as_ref()?;
                if !p.exists() {
                    return None;
                }
                Some(e.start_ms / 1000.0 + wav_duration_s(p))
            })
            .fold(0.0_f64, f64::max);
        let total_s = audio_end.max(bgm_end).max(se_end);
        let total_ticks = (total_s * 30000.0) as u64;

        let resources = self.build_resource_tags();
        let clips = self.build_audio_clips();

        let format_name = self.fcpxml_fps.format_name();
        let frame_duration = self.fcpxml_fps.frame_duration_attr();
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.8">
    <resources>
        <format id="r1" name="{format_name}" frameDuration="{frame_duration}"/>
        {resources}
    </resources>
    <library>
        <event name="VoiceProduction">
            <project name="ScriptTimeline">
                <sequence format="r1" duration="{total_ticks}/30000s">
                    <spine>
                        <gap name="Gap" offset="0s" duration="{total_ticks}/30000s">
                            {clips}
                        </gap>
                    </spine>
                </sequence>
            </project>
        </event>
    </library>
</fcpxml>"#
        );

        std::fs::write(&path, xml.as_bytes())?;
        info!("FCPXML exported to: {}", path.display());
        Ok(())
    }

    pub fn generate_combined_audio(&self, suffix: &str) -> anyhow::Result<()> {
        let out_path = with_suffix(&self.output_dir.join("full_dialogue.wav"), suffix);
        let sr = self.sample_rate;
        let se_fade_s = self.bgm_config.se_fade_out_s;

        // dialogue クリップを収集
        let audio_events: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.event_type == EventType::Audio)
            .filter(|e| e.path.as_ref().map(|p| p.exists()).unwrap_or(false))
            .collect();
        let bgm_segs = self.compute_bgm_segments();
        let se_events: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.event_type == EventType::Se)
            .filter(|e| e.path.as_ref().map(|p| p.exists()).unwrap_or(false))
            .collect();

        // 音声・BGM・SE のいずれも存在しない場合のみスキップ (Python版 Fix #5 相当)
        if audio_events.is_empty() && bgm_segs.is_empty() && se_events.is_empty() {
            warn!("ミックス対象の音声・BGM・SEが存在しません。full_dialogue.wav の生成をスキップします。");
            return Ok(());
        }

        info!(
            "voice={} bgm={} se={} をミックス中...",
            audio_events.len(),
            bgm_segs.len(),
            se_events.len()
        );

        // 1. dialogue クリップを読み込み、総サンプル数を算出
        let mut total_samples: usize = 0;
        let mut clips: Vec<(usize, Vec<[f32; 2]>)> = Vec::new();
        for event in &audio_events {
            let path = event.path.as_ref().unwrap();
            let start = (event.start_ms / 1000.0 * sr as f64) as usize;
            match read_stereo_float(path, sr) {
                Ok(samples) => {
                    total_samples = total_samples.max(start + samples.len());
                    clips.push((start, samples));
                }
                Err(e) => warn!("読み込みスキップ: {} ({e})", path.display()),
            }
        }

        // BGM のバッファ占有サンプルを反映 (クロスフェード拡張分を含む)
        for seg in &bgm_segs {
            let end_s = ((seg.mix_start_s + seg.mix_duration_s) * sr as f64) as usize;
            total_samples = total_samples.max(end_s);
        }

        // SE のバッファ占有サンプルを反映
        for event in &se_events {
            let path = event.path.as_ref().unwrap();
            let end_s = ((event.start_ms / 1000.0 + wav_duration_s(path)) * sr as f64) as usize;
            total_samples = total_samples.max(end_s);
        }

        if total_samples == 0 {
            warn!("有効なサンプルがありません。");
            return Ok(());
        }

        // 2. 出力バッファを作成し、dialogue クリップを加算
        let mut buf: Vec<[f32; 2]> = vec![[0.0, 0.0]; total_samples];
        for (start, samples) in clips {
            for (i, s) in samples.iter().enumerate() {
                if start + i < buf.len() {
                    buf[start + i][0] += s[0];
                    buf[start + i][1] += s[1];
                }
            }
        }

        // 3. BGM をクロスフェード付きでループ展開してミックス (-10dB相当の0.3倍, Python版 Fix #1相当)
        for seg in &bgm_segs {
            if !seg.path.exists() {
                continue;
            }
            let Ok(bgm) = read_stereo_float(&seg.path, sr) else {
                continue;
            };
            if bgm.is_empty() {
                continue;
            }
            let need = (seg.mix_duration_s * sr as f64) as usize;
            if need == 0 {
                continue;
            }
            let mut looped = loop_to_length(&bgm, need);

            let fi_n = ((seg.fade_in_s * sr as f64) as usize).min(looped.len());
            for (i, s) in looped[..fi_n].iter_mut().enumerate() {
                let g = i as f32 / fi_n.max(1) as f32;
                s[0] *= g;
                s[1] *= g;
            }
            let fo_n = ((seg.fade_out_s * sr as f64) as usize).min(looped.len());
            let fo_start = looped.len() - fo_n;
            for (i, s) in looped[fo_start..].iter_mut().enumerate() {
                let g = 1.0 - (i as f32 / fo_n.max(1) as f32);
                s[0] *= g;
                s[1] *= g;
            }

            let start_s = (seg.mix_start_s * sr as f64) as usize;
            if start_s < total_samples {
                let end_s = (start_s + looped.len()).min(total_samples);
                for (i, s) in looped[..end_s - start_s].iter().enumerate() {
                    buf[start_s + i][0] += s[0] * 0.3;
                    buf[start_s + i][1] += s[1] * 0.3;
                }
                info!(
                    "BGMをミックス: {} ({:.1}s, fi={:.2}s, fo={:.2}s)",
                    seg.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    seg.mix_duration_s,
                    seg.fade_in_s,
                    seg.fade_out_s,
                );
            }
        }

        // 4. SE をミックス (末尾フェードアウト付き)
        for event in &se_events {
            let path = event.path.as_ref().unwrap();
            let Ok(mut se) = read_stereo_float(path, sr) else {
                continue;
            };
            let fo_n = ((se_fade_s * sr as f64) as usize).min(se.len());
            if fo_n > 0 {
                let fo_start = se.len() - fo_n;
                for (i, s) in se[fo_start..].iter_mut().enumerate() {
                    let g = 1.0 - (i as f32 / fo_n as f32);
                    s[0] *= g;
                    s[1] *= g;
                }
            }
            let start_s = (event.start_ms / 1000.0 * sr as f64) as usize;
            if start_s < total_samples {
                let end_s = (start_s + se.len()).min(total_samples);
                for (i, s) in se[..end_s - start_s].iter().enumerate() {
                    buf[start_s + i][0] += s[0];
                    buf[start_s + i][1] += s[1];
                }
                info!(
                    "SEをミックス: {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                );
            }
        }

        // 5. クリッピング防止
        let peak = buf
            .iter()
            .flat_map(|s| s.iter())
            .cloned()
            .map(f32::abs)
            .fold(0.0_f32, f32::max);
        if peak > 1.0 {
            buf.iter_mut().for_each(|s| {
                s[0] /= peak;
                s[1] /= peak;
            });
        }

        // 6. WAV 書き出し
        std::fs::create_dir_all(out_path.parent().unwrap_or(Path::new(".")))?;
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&out_path, spec)?;
        for frame in &buf {
            writer.write_sample((frame[0] * 32767.0) as i16)?;
            writer.write_sample((frame[1] * 32767.0) as i16)?;
        }
        writer.finalize()?;
        info!("ミックス音声を出力しました: {}", out_path.display());
        Ok(())
    }

    fn build_resource_tags(&self) -> String {
        self.events
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let p = e.path.as_ref()?;
                let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
                let uri = file_uri(&abs);
                let name = p.file_name()?.to_string_lossy();
                Some(format!(
                    r#"<asset id="a{i}" name="{}" src="{}"/>"#,
                    xml_escape(&name),
                    xml_escape(&uri),
                ))
            })
            .collect::<Vec<_>>()
            .join("\n        ")
    }

    fn build_audio_clips(&self) -> String {
        let bgm_segs = self.compute_bgm_segments();
        self.events.iter().enumerate()
            .filter_map(|(i, e)| match e.event_type {
                EventType::Audio => {
                    let start = (e.start_ms / 1000.0 * 30000.0) as u64;
                    let dur = (e.duration_ms / 1000.0 * 30000.0) as u64;
                    Some(format!(
                        r#"<audio ref="a{i}" lane="{}" offset="{start}/30000s" duration="{dur}/30000s" role="dialogue"/>"#,
                        i + 1
                    ))
                }
                EventType::BgmStart => {
                    // Python版 exporter.py:218-223: 実際の bgm_start〜bgm_stop 区間長を使う
                    let seg = bgm_segs.iter().find(|s| s.index == i)?;
                    if seg.event_duration_s <= 0.0 {
                        return None;
                    }
                    let start = (seg.event_start_s * 30000.0) as u64;
                    let dur = (seg.event_duration_s * 30000.0) as u64;
                    Some(format!(
                        r#"<audio ref="a{i}" lane="{}" offset="{start}/30000s" duration="{dur}/30000s" role="music"/>"#,
                        i + 1
                    ))
                }
                EventType::Se => {
                    // Python版 exporter.py:224-229: 実ファイル長を使う
                    let path = e.path.as_ref()?;
                    let dur_s = wav_duration_s(path);
                    if dur_s <= 0.0 {
                        return None;
                    }
                    let start = (e.start_ms / 1000.0 * 30000.0) as u64;
                    let dur = (dur_s * 30000.0) as u64;
                    Some(format!(
                        r#"<audio ref="a{i}" lane="{}" offset="{start}/30000s" duration="{dur}/30000s" role="effects"/>"#,
                        i + 1
                    ))
                }
                EventType::BgmStop => None,
                EventType::Paragraph => None,
            })
            .collect::<Vec<_>>()
            .join("\n                            ")
    }

    /// `#bgm_start`/`#bgm_stop` のペアからミックス用セグメント情報を計算する
    /// (Python版 `_compute_bgm_segments` 相当)。
    fn compute_bgm_segments(&self) -> Vec<BgmSegment> {
        let xfade = self.bgm_config.crossfade_s;
        let half = xfade / 2.0;

        let total_s = self
            .events
            .iter()
            .map(|e| (e.start_ms + e.duration_ms) / 1000.0)
            .fold(0.0_f64, f64::max);

        struct Raw {
            index: usize,
            path: PathBuf,
            event_start: f64,
            event_end: f64,
        }

        let mut raw: Vec<Raw> = Vec::new();
        let mut pending: Option<(usize, f64, PathBuf)> = None;

        for (i, event) in self.events.iter().enumerate() {
            match event.event_type {
                EventType::BgmStart => {
                    if let Some((idx, start, path)) = pending.take() {
                        raw.push(Raw {
                            index: idx,
                            path,
                            event_start: start,
                            event_end: event.start_ms / 1000.0,
                        });
                    }
                    pending = Some((
                        i,
                        event.start_ms / 1000.0,
                        event.path.clone().unwrap_or_default(),
                    ));
                }
                EventType::BgmStop => {
                    if let Some((idx, start, path)) = pending.take() {
                        raw.push(Raw {
                            index: idx,
                            path,
                            event_start: start,
                            event_end: event.start_ms / 1000.0,
                        });
                    }
                }
                _ => {}
            }
        }
        if let Some((idx, start, path)) = pending {
            let mut event_end = total_s;
            if event_end <= start {
                let file_dur = wav_duration_s(&path);
                event_end = start + if file_dur > 0.0 { file_dur } else { 30.0 };
            }
            raw.push(Raw {
                index: idx,
                path,
                event_start: start,
                event_end,
            });
        }

        if raw.is_empty() {
            return Vec::new();
        }

        let n = raw.len();
        raw.into_iter()
            .enumerate()
            .map(|(k, seg)| {
                let seg_dur = (seg.event_end - seg.event_start).max(0.0);
                let clamp = seg_dur / 3.0;
                let fi_half = if k > 0 { half.min(clamp) } else { 0.0 };
                let fo_half = if k < n - 1 { half.min(clamp) } else { 0.0 };
                let mix_start = (seg.event_start - fi_half).max(0.0);
                let mix_end = seg.event_end + fo_half;
                BgmSegment {
                    index: seg.index,
                    path: seg.path,
                    event_start_s: seg.event_start,
                    event_duration_s: seg_dur,
                    mix_start_s: mix_start,
                    mix_duration_s: (mix_end - mix_start).max(0.0),
                    fade_in_s: fi_half * 2.0,
                    fade_out_s: fo_half * 2.0,
                }
            })
            .collect()
    }
}

/// timeline.json に書き出すイベント表現。パスは project_dir 相対・スラッシュ区切りに正規化する。
#[derive(serde::Serialize)]
struct JsonEvent {
    event_type: EventType,
    start_ms: f64,
    duration_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cast: Option<String>,
}

/// project_dir 配下ならその相対パスを、外部ならフルパスを、いずれもスラッシュ区切りで返す。
fn rel_path_string(path: &Path, base: &Path) -> String {
    let p = path.strip_prefix(base).unwrap_or(path);
    p.to_string_lossy().replace('\\', "/")
}

/// ファイル名の拡張子の前に suffix を挿入する。suffix が空ならパスをそのまま返す。
/// 例: with_suffix("voice_0001.wav", "_3") == "voice_0001_3.wav"
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem}{suffix}.{}", ext.to_string_lossy()),
        None => format!("{stem}{suffix}"),
    };
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(name),
        _ => PathBuf::from(name),
    }
}

/// パスが書き込み可能か（=使用中でないか）。非存在は true。
/// 既存ファイルは truncate せずに書き込みオープンを試し、成否で判定する。
pub fn is_path_writable(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

/// `resolve_generation_suffix` が返す、確保した世代サフィックスの占有を表すガード。
/// Drop時にロックファイルを削除し、そのsuffixを他プロセスへ解放する。
/// 生成が完了する（または失敗する）まで保持し続けること。
pub struct GenerationLock {
    lock_path: PathBuf,
    /// ロックの保有を表すファイルハンドル。開いている間は他プロセスから
    /// 「持ち主がいる」と見える（`reclaim_if_unowned` 参照）。
    file: Option<std::fs::File>,
}

impl Drop for GenerationLock {
    fn drop(&mut self) {
        // 先にハンドルを閉じる。残骸から奪い返したロックは共有モード0で開いており、
        // ハンドルが開いたままでは Windows で削除できない。
        self.file.take();
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn lock_path_for(lock_dir: &Path, suffix: &str) -> PathBuf {
    lock_dir.join(format!(".s2v_generation{suffix}.lock"))
}

/// 既存のロックファイルが「持ち主のいない残骸」なら開き直して所有権を奪う。
///
/// Ctrl+C はハンドリングして解放するようにしたが、タスクマネージャからの強制終了・
/// GUI の × 終了・電源断ではデストラクタが走らずロックファイルだけが残る。残骸を
/// 検出できないと、以降の実行がずっと `_1`,`_2`... へフォールバックし続け、
/// 出力ファイル名が永久にずれていく。
///
/// Windows では共有モード 0（=他のハンドルを一切許さない）でのオープンが、
/// 他プロセスがそのファイルを開いている間は共有違反で必ず失敗する。
/// つまり「開けた = 誰も掴んでいない = 残骸」と判定できる。
#[cfg(windows)]
fn reclaim_if_unowned(lock_path: &Path) -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .share_mode(0)
        .open(lock_path)
        .ok()
}

/// Windows 以外には「他プロセスがファイルを開いているか」を標準ライブラリだけで
/// 判定する手段がないため、残骸の奪い返しは行わない（従来どおり連番へ回避する）。
#[cfg(not(windows))]
fn reclaim_if_unowned(_lock_path: &Path) -> Option<std::fs::File> {
    None
}

/// 生成の既定名ファイル一式から世代サフィックスを決め、そのサフィックスを排他的に確保する。
///
/// `exists()`による事前チェックだけでは、チェックしてから実際にファイルを書き終えるまでの間に
/// 別プロセスが同じ台本を処理して同じsuffixを選べてしまう(TOCTOU)。ここでは候補ごとに
/// ロックファイルを`create_new`でアトミックに作成することで排他制御する
/// （`create_new`はファイルが既に存在すればOS側で必ず失敗するため、2プロセスが
/// 同じsuffixの確保に同時に成功することはない）。
///
/// すべて書込可なら ""。いずれか使用中なら、一式の `_n` 版がすべて未存在かつ
/// ロック確保に成功する最小の `_n`。
pub fn resolve_generation_suffix(
    default_files: &[PathBuf],
    lock_dir: &Path,
    max: usize,
) -> anyhow::Result<(String, GenerationLock)> {
    fn try_claim(lock_dir: &Path, suffix: &str) -> anyhow::Result<Option<GenerationLock>> {
        let lock_path = lock_path_for(lock_dir, suffix);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => Ok(Some(GenerationLock {
                lock_path,
                file: Some(file),
            })),
            // 既にロックがある。生存プロセスが保持しているなら諦めるが、
            // 強制終了で残った残骸なら奪い返す。
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(reclaim_if_unowned(&lock_path).map(|file| GenerationLock {
                    lock_path,
                    file: Some(file),
                }))
            }
            Err(e) => Err(e.into()),
        }
    }

    let needs_fallback = default_files
        .iter()
        .any(|p| p.exists() && !is_path_writable(p));
    if !needs_fallback {
        // 既存の同名ファイルが混じっていても、すべて書込可なら"" のまま上書きを許す
        // （既存の仕様。ロック確保にだけ失敗した場合は _n フォールバックへ進む）。
        if let Some(guard) = try_claim(lock_dir, "")? {
            return Ok((String::new(), guard));
        }
    }
    for n in 1..=max {
        let suffix = format!("_{n}");
        if !default_files
            .iter()
            .all(|p| !with_suffix(p, &suffix).exists())
        {
            continue;
        }
        if let Some(guard) = try_claim(lock_dir, &suffix)? {
            return Ok((suffix, guard));
        }
    }
    anyhow::bail!("使用中の出力を回避する空き連番({max}まで)が見つかりませんでした")
}

/// BGMミックス用セグメント情報 (Python版 `_compute_bgm_segments` の戻り値相当)
struct BgmSegment {
    index: usize,
    path: PathBuf,
    event_start_s: f64,
    event_duration_s: f64,
    mix_start_s: f64,
    mix_duration_s: f64,
    fade_in_s: f64,
    fade_out_s: f64,
}

/// WAV ファイルの再生時間 (秒)。読み込み失敗時は 0.0 (Python版 `_get_file_duration_s` 相当)
fn wav_duration_s(path: &Path) -> f64 {
    let Ok(reader) = hound::WavReader::open(path) else {
        return 0.0;
    };
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return 0.0;
    }
    reader.duration() as f64 / spec.sample_rate as f64
}

/// `src` を `need` サンプルになるまでループして返す (Python版 `_loop_to_length` 相当)
fn loop_to_length(src: &[[f32; 2]], need: usize) -> Vec<[f32; 2]> {
    if src.is_empty() || need == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(need);
    while out.len() < need {
        let take = (need - out.len()).min(src.len());
        out.extend_from_slice(&src[..take]);
    }
    out
}

fn format_srt_time(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0) as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1_000;
    let ms = total_ms % 1_000;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

/// 絶対パスを `file://` URI にする。
///
/// Windows の `canonicalize` は `\\?\D:\...`（UNC なら `\\?\UNC\server\share\...`）という
/// verbatim パスを返すので、接頭辞を外してから `file:///D:/...` / `file://server/share/...` にする。
fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    };
    let s = s.replace('\\', "/");
    if let Some(unc) = s.strip_prefix("//") {
        format!("file://{unc}")
    } else if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

fn read_stereo_float(path: &Path, target_sr: u32) -> anyhow::Result<Vec<[f32; 2]>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.unwrap() as f32 / max)
                .collect()
        }
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
    };
    let stereo: Vec<[f32; 2]> = if spec.channels == 1 {
        raw.iter().map(|&s| [s, s]).collect()
    } else if spec.channels == 2 {
        raw.chunks(2).map(|c| [c[0], c[1]]).collect()
    } else {
        raw.chunks(spec.channels as usize)
            .map(|c| [c[0], c[1]])
            .collect()
    };
    // 市販の BGM/SE は 44.1kHz が多い。最近傍法だとエイリアシングが出るため、
    // 台詞と同じ帯域制限付きリサンプラで左右別々に変換する。
    if spec.sample_rate != target_sr {
        let left: Vec<f32> = stereo.iter().map(|s| s[0]).collect();
        let right: Vec<f32> = stereo.iter().map(|s| s[1]).collect();
        let left = s2v_audio::resample_mono(&left, spec.sample_rate, target_sr)?;
        let right = s2v_audio::resample_mono(&right, spec.sample_rate, target_sr)?;
        Ok(left.into_iter().zip(right).map(|(l, r)| [l, r]).collect())
    } else {
        Ok(stereo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::EventType;
    use std::path::PathBuf;

    fn make_audio_event(
        start_ms: f64,
        duration_ms: f64,
        text: &str,
        path: Option<PathBuf>,
    ) -> TimelineEvent {
        TimelineEvent {
            event_type: EventType::Audio,
            start_ms,
            duration_ms,
            path,
            text: Some(text.to_string()),
            display_text: Some(text.to_string()),
            cast: Some("テスト".to_string()),
            name: None,
        }
    }

    fn make_paragraph_event(start_ms: f64) -> TimelineEvent {
        TimelineEvent {
            event_type: EventType::Paragraph,
            start_ms,
            duration_ms: 0.0,
            path: None,
            text: None,
            display_text: Some("[PARAGRAPH]".to_string()),
            cast: None,
            name: None,
        }
    }

    fn make_named_paragraph_event(start_ms: f64, name: &str) -> TimelineEvent {
        TimelineEvent {
            event_type: EventType::Paragraph,
            start_ms,
            duration_ms: 0.0,
            path: None,
            text: None,
            display_text: Some(format!("[PARAGRAPH {name}]")),
            cast: None,
            name: Some(name.to_string()),
        }
    }

    fn default_bgm() -> BgmConfig {
        BgmConfig {
            crossfade_s: 3.0,
            se_fade_out_s: 0.05,
        }
    }

    fn write_wav(path: &Path, sr: u32, seconds: f32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let n = (sr as f32 * seconds) as usize;
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..n {
            let v = ((i as f32 / sr as f32) * 440.0 * 2.0 * std::f32::consts::PI).sin();
            w.write_sample((v * 8000.0) as i16).unwrap();
            w.write_sample((v * 8000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn srt_generates_correct_format() {
        let events = vec![
            make_audio_event(0.0, 1500.0, "こんにちは", None),
            make_audio_event(2000.0, 800.0, "さようなら", None),
        ];
        let dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, dir.path(), 48000, default_bgm());
        exp.generate_srt("").unwrap();

        let content = std::fs::read_to_string(dir.path().join("timeline/subtitles.srt")).unwrap();
        assert!(content.contains("1\n"));
        assert!(content.contains("00:00:00,000 --> 00:00:01,500"));
        assert!(content.contains("こんにちは"));
        assert!(content.contains("2\n"));
        assert!(content.contains("00:00:02,000 --> 00:00:02,800"));
        assert!(content.contains("さようなら"));
    }

    #[test]
    fn srt_includes_paragraph_markers_in_chronological_order_with_continuous_numbering() {
        let events = vec![
            make_audio_event(0.0, 1500.0, "こんにちは", None),
            make_paragraph_event(1500.0),
            make_audio_event(3000.0, 800.0, "さようなら", None),
        ];
        let dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, dir.path(), 48000, default_bgm());
        exp.generate_srt("").unwrap();

        let content = std::fs::read_to_string(dir.path().join("timeline/subtitles.srt")).unwrap();
        // 1: 通常の字幕
        assert!(content.contains("1\n00:00:00,000 --> 00:00:01,500\nこんにちは\n"));
        // 2: ゼロ秒の [PARAGRAPH] エントリ。タイムスタンプは直前のセリフの終了時刻と同一
        assert!(content.contains("2\n00:00:01,500 --> 00:00:01,500\n[PARAGRAPH]\n"));
        // 3: 通常の字幕（連番が続く）
        assert!(content.contains("3\n00:00:03,000 --> 00:00:03,800\nさようなら\n"));
    }

    #[test]
    fn srt_writes_named_paragraph_marker() {
        let dir = tempfile::tempdir().unwrap();
        let events = vec![make_named_paragraph_event(1500.0, "オープニング")];
        Exporter::new(&events, dir.path(), 48000, default_bgm())
            .generate_srt("")
            .unwrap();
        let content = std::fs::read_to_string(dir.path().join("timeline/subtitles.srt")).unwrap();
        assert!(
            content.contains("[PARAGRAPH オープニング]"),
            "実際の内容: {content}"
        );
    }

    #[test]
    fn srt_time_format() {
        assert_eq!(format_srt_time(0.0), "00:00:00,000");
        assert_eq!(format_srt_time(1.5), "00:00:01,500");
        assert_eq!(format_srt_time(3661.1), "01:01:01,100");
        assert_eq!(format_srt_time(3600.0), "01:00:00,000");
    }

    #[test]
    fn fcpxml_generates_valid_structure() {
        let events = vec![make_audio_event(0.0, 2000.0, "テスト", None)];
        let dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, dir.path(), 48000, default_bgm());
        exp.generate_fcpxml("").unwrap();

        let content = std::fs::read_to_string(dir.path().join("timeline/timeline.fcpxml")).unwrap();
        assert!(content.contains(r#"<fcpxml version="1.8">"#));
        assert!(content.contains("<resources>"));
        assert!(content.contains("<library>"));
        assert!(content.contains("dialogue"));
    }

    /// 字幕ズレ調査(Obsidian記録)により、Filmoraの30fps設定でSRT/WAVとの累積ドリフトが
    /// 消えることが確定している。FCPXML経路だけ29.97fpsを自ら宣言していると同じドリフトを
    /// 再導入するため、既定は30fpsにする(29.97が必要な場合は明示的に指定する)。
    #[test]
    fn fcpxml_defaults_to_30fps_matching_srt_wav_timeline_basis() {
        let events = vec![make_audio_event(0.0, 2000.0, "テスト", None)];
        let dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, dir.path(), 48000, default_bgm());
        exp.generate_fcpxml("").unwrap();

        let content = std::fs::read_to_string(dir.path().join("timeline/timeline.fcpxml")).unwrap();
        assert!(
            content.contains(r#"name="FFVideoFormat1080p30""#),
            "既定は30fpsのフォーマット名であるべき: {content}"
        );
        assert!(
            content.contains(r#"frameDuration="1000/30000s""#),
            "既定は正確な30fps(1000/30000s)であるべき: {content}"
        );
        assert!(
            !content.contains("2997"),
            "既定では29.97fpsを名乗ってはいけない: {content}"
        );
    }

    #[test]
    fn fcpxml_can_opt_into_2997fps_explicitly() {
        let events = vec![make_audio_event(0.0, 2000.0, "テスト", None)];
        let dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, dir.path(), 48000, default_bgm())
            .with_fcpxml_fps(FrameRate::Fps2997);
        exp.generate_fcpxml("").unwrap();

        let content = std::fs::read_to_string(dir.path().join("timeline/timeline.fcpxml")).unwrap();
        assert!(content.contains(r#"name="FFVideoFormat1080p2997""#));
        assert!(content.contains(r#"frameDuration="1001/30000s""#));
    }

    #[test]
    fn combined_audio_produces_wav() {
        let dir = tempfile::tempdir().unwrap();
        let wav1 = dir.path().join("a1.wav");
        let wav2 = dir.path().join("a2.wav");
        write_wav(&wav1, 48000, 0.1);
        write_wav(&wav2, 48000, 0.1);

        let events = vec![
            make_audio_event(0.0, 100.0, "A", Some(wav1)),
            make_audio_event(200.0, 100.0, "B", Some(wav2)),
        ];
        let out_dir = dir.path().join("out");
        let exp = Exporter::new(&events, &out_dir, 48000, default_bgm());
        exp.generate_combined_audio("").unwrap();

        let out = out_dir.join("full_dialogue.wav");
        assert!(out.exists());
        let reader = hound::WavReader::open(&out).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 48000);
        assert!(reader.spec().bits_per_sample == 16);
    }

    #[test]
    fn combined_audio_mix_is_louder_at_clip_start() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("a.wav");
        write_wav(&wav, 48000, 0.2);

        let events = vec![make_audio_event(500.0, 200.0, "A", Some(wav))];
        let out_dir = dir.path().join("out");
        let exp = Exporter::new(&events, &out_dir, 48000, default_bgm());
        exp.generate_combined_audio("").unwrap();

        let out = out_dir.join("full_dialogue.wav");
        let mut reader = hound::WavReader::open(&out).unwrap();
        let all: Vec<i16> = reader.samples().map(|s| s.unwrap()).collect();

        let silence_end = (0.5 * 48000.0 * 2.0) as usize; // 0.5s offset
        let silence_max = all[..silence_end]
            .iter()
            .map(|&s| s.unsigned_abs())
            .max()
            .unwrap_or(0);
        let sound_start = silence_end;
        let sound_max = all[sound_start..]
            .iter()
            .map(|&s| s.unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(silence_max == 0, "silence section should be zero");
        assert!(sound_max > 0, "sound section should be non-zero");
    }

    fn make_bgm_start(start_ms: f64, path: PathBuf) -> TimelineEvent {
        TimelineEvent {
            event_type: EventType::BgmStart,
            start_ms,
            duration_ms: 0.0,
            path: Some(path),
            text: None,
            display_text: None,
            cast: None,
            name: None,
        }
    }

    fn make_bgm_stop(start_ms: f64) -> TimelineEvent {
        TimelineEvent {
            event_type: EventType::BgmStop,
            start_ms,
            duration_ms: 0.0,
            path: None,
            text: None,
            display_text: None,
            cast: None,
            name: None,
        }
    }

    fn make_se(start_ms: f64, path: PathBuf) -> TimelineEvent {
        TimelineEvent {
            event_type: EventType::Se,
            start_ms,
            duration_ms: 0.0,
            path: Some(path),
            text: None,
            display_text: None,
            cast: None,
            name: None,
        }
    }

    #[test]
    fn combined_audio_mixes_bgm_into_output() {
        // Python版 generate_combined_audio はBGMをループ・クロスフェードしつつ0.3倍でミックスする。
        // セリフが無くてもBGM単独でミックス音声が生成され、無音であってはならない。
        let dir = tempfile::tempdir().unwrap();
        let bgm = dir.path().join("bgm.wav");
        write_wav(&bgm, 48000, 0.5);

        let events = vec![make_bgm_start(0.0, bgm.clone()), make_bgm_stop(1000.0)];
        let out_dir = dir.path().join("out");
        let exp = Exporter::new(&events, &out_dir, 48000, default_bgm());
        exp.generate_combined_audio("").unwrap();

        let out = out_dir.join("full_dialogue.wav");
        assert!(out.exists(), "BGMのみでもfull_dialogue.wavが生成されるはず");
        let mut reader = hound::WavReader::open(&out).unwrap();
        let all: Vec<i16> = reader.samples().map(|s| s.unwrap()).collect();
        let max = all.iter().map(|&s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(
            max > 0,
            "BGMがミックス出力に含まれているはず (Python版は0.3倍でミックスする)"
        );
    }

    #[test]
    fn combined_audio_mixes_se_into_output() {
        // Python版 generate_combined_audio はSEをフェードアウト付きでミックスする。
        let dir = tempfile::tempdir().unwrap();
        let se = dir.path().join("se.wav");
        write_wav(&se, 48000, 0.2);

        let events = vec![make_se(100.0, se.clone())];
        let out_dir = dir.path().join("out");
        let exp = Exporter::new(&events, &out_dir, 48000, default_bgm());
        exp.generate_combined_audio("").unwrap();

        let out = out_dir.join("full_dialogue.wav");
        assert!(out.exists(), "SEのみでもfull_dialogue.wavが生成されるはず");
        let mut reader = hound::WavReader::open(&out).unwrap();
        let all: Vec<i16> = reader.samples().map(|s| s.unwrap()).collect();
        let max = all.iter().map(|&s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(max > 0, "SEがミックス出力に含まれているはず");
    }

    #[test]
    fn fcpxml_bgm_clip_duration_reflects_actual_event_span_not_crossfade() {
        // Python版 exporter.py:218-223 は _compute_bgm_segments の event_duration
        // (実際の bgm_start〜bgm_stop 区間) をクリップ長として使う。
        // クロスフェード長(既定3.0秒)固定であってはならない。
        let dir = tempfile::tempdir().unwrap();
        let bgm = dir.path().join("bgm.wav");
        write_wav(&bgm, 48000, 0.1);

        let events = vec![
            make_bgm_start(0.0, bgm.clone()),
            make_bgm_stop(5000.0), // 実区間5.0秒 = 150000 ticks (クロスフェード3.0秒=90000ticksとは異なる)
        ];
        let out_dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, out_dir.path(), 48000, default_bgm());
        exp.generate_fcpxml("").unwrap();
        let content =
            std::fs::read_to_string(out_dir.path().join("timeline/timeline.fcpxml")).unwrap();

        assert!(
            content.contains(r#"duration="150000/30000s" role="music""#),
            "BGMクリップ長は実区間(5.0s=150000ticks)であるべき。実際の出力:\n{content}"
        );
    }

    #[test]
    fn fcpxml_se_clip_duration_reflects_actual_file_length() {
        // Python版 exporter.py:224-229 は _get_file_duration_s で実ファイル長を取得する。
        // 固定 0.05秒(1500 ticks) であってはならない。
        let dir = tempfile::tempdir().unwrap();
        let se = dir.path().join("se.wav");
        write_wav(&se, 48000, 0.4); // 0.4秒 = 12000 ticks

        let events = vec![make_se(0.0, se.clone())];
        let out_dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, out_dir.path(), 48000, default_bgm());
        exp.generate_fcpxml("").unwrap();
        let content =
            std::fs::read_to_string(out_dir.path().join("timeline/timeline.fcpxml")).unwrap();

        assert!(
            content.contains(r#"duration="12000/30000s" role="effects""#),
            "SEクリップ長は実ファイル長(0.4s=12000ticks)であるべき。実際の出力:\n{content}"
        );
    }

    #[test]
    fn fcpxml_total_duration_accounts_for_se_file_extent_beyond_last_event_start() {
        // Python版 exporter.py:33-47 は se_end = event['start'] + 実ファイル長 を考慮し、
        // total_duration = max(audio_end, bgm_end, se_end) とする。
        // SEイベントは登録時 duration_ms=0 のため、開始時刻だけでなく
        // 実ファイル長を加味しないと、SEがダイアローグより後まで鳴る場合に
        // タイムラインが途中で切れてしまう。
        let dir = tempfile::tempdir().unwrap();
        let se = dir.path().join("se.wav");
        write_wav(&se, 48000, 5.0); // 5秒のSE

        let events = vec![
            make_audio_event(0.0, 500.0, "短いセリフ", None), // 0.5sで終わる
            make_se(1000.0, se.clone()),                      // 1.0s開始、5秒再生 -> 6.0sまで
        ];
        let out_dir = tempfile::tempdir().unwrap();
        let exp = Exporter::new(&events, out_dir.path(), 48000, default_bgm());
        exp.generate_fcpxml("").unwrap();
        let content =
            std::fs::read_to_string(out_dir.path().join("timeline/timeline.fcpxml")).unwrap();

        let dur_str = content
            .split(r#"sequence format="r1" duration=""#)
            .nth(1)
            .unwrap()
            .split("/30000s")
            .next()
            .unwrap();
        let total_ticks: u64 = dur_str.parse().unwrap();
        assert!(
            total_ticks >= 180_000,
            "全体長はSEの終端(1.0s開始+5.0秒=6.0s=180000ticks)を含むはず。実際: {total_ticks} ticks"
        );
    }

    #[test]
    fn empty_audio_events_skips_wav_creation() {
        let dir = tempfile::tempdir().unwrap();
        let events = vec![
            make_audio_event(0.0, 1000.0, "テスト", None), // path=None
        ];
        let out_dir = dir.path().join("out");
        let exp = Exporter::new(&events, &out_dir, 48000, default_bgm());
        exp.generate_combined_audio("").unwrap();
        assert!(!out_dir.join("full_dialogue.wav").exists());
    }

    #[test]
    fn with_suffix_inserts_before_extension() {
        assert_eq!(
            with_suffix(Path::new("a/voice_0001.wav"), "_3"),
            PathBuf::from("a/voice_0001_3.wav")
        );
        assert_eq!(
            with_suffix(Path::new("subtitles.srt"), "_2"),
            PathBuf::from("subtitles_2.srt")
        );
        assert_eq!(
            with_suffix(Path::new("noext"), "_1"),
            PathBuf::from("noext_1")
        );
        assert_eq!(with_suffix(Path::new("x.wav"), ""), PathBuf::from("x.wav"));
    }

    #[test]
    fn is_path_writable_true_for_missing_and_normal_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_path_writable(&dir.path().join("nope.wav")));
        let f = dir.path().join("ok.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(is_path_writable(&f));
    }

    #[test]
    fn is_path_writable_false_for_directory_named_like_file() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("locked.wav");
        std::fs::create_dir(&d).unwrap();
        assert!(!is_path_writable(&d));
    }

    #[test]
    fn resolve_suffix_empty_when_all_writable() {
        let dir = tempfile::tempdir().unwrap();
        // 非存在のみ → 書込可扱い → ""
        let files = vec![dir.path().join("a.wav"), dir.path().join("b.srt")];
        assert_eq!(
            resolve_generation_suffix(&files, dir.path(), 100)
                .unwrap()
                .0,
            ""
        );
        // 既存かつ書込可のファイルが混じっていても fallback しない（exists()&&writable のパスを検証）
        let existing = dir.path().join("a.wav");
        std::fs::write(&existing, b"x").unwrap();
        let files2 = vec![existing, dir.path().join("b.srt")];
        assert_eq!(
            resolve_generation_suffix(&files2, dir.path(), 100)
                .unwrap()
                .0,
            ""
        );
    }

    #[test]
    fn resolve_suffix_falls_back_when_one_locked() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        std::fs::create_dir(&a).unwrap(); // a.wav をディレクトリにして書込不可(=ロック相当)
        let b = dir.path().join("b.srt");
        let files = vec![a, b];
        assert_eq!(
            resolve_generation_suffix(&files, dir.path(), 100)
                .unwrap()
                .0,
            "_1"
        );
    }

    #[test]
    fn resolve_suffix_skips_existing_numbered_slot() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        std::fs::create_dir(&a).unwrap();
        let b = dir.path().join("b.srt");
        std::fs::write(&b, b"x").unwrap();
        std::fs::write(dir.path().join("a_1.wav"), b"x").unwrap(); // _1 スロットを一部埋める
        let files = vec![a, b];
        assert_eq!(
            resolve_generation_suffix(&files, dir.path(), 100)
                .unwrap()
                .0,
            "_2"
        );
    }

    /// TOCTOU再現: 1回目の呼び出しでガードを保持したまま(=まだ生成完了・cleanup前)
    /// 2回目を呼ぶと、ファイル自体はまだ存在しない(exists()チェックだけでは""に見える)にも
    /// 関わらず、ロックにより別のsuffixへフォールバックするべき。
    /// ロック解放後は再び""が使えるようになる。
    #[test]
    fn resolve_suffix_atomically_claims_slot_preventing_concurrent_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![dir.path().join("a.wav"), dir.path().join("b.srt")];

        let (first_suffix, first_guard) =
            resolve_generation_suffix(&files, dir.path(), 100).unwrap();
        assert_eq!(first_suffix, "");

        // 1回目のガードをまだ保持している間に2回目を呼ぶ = 並行実行のシミュレーション。
        // 出力ファイル自体はまだ1つも書かれていない(existsチェックだけなら両方""を返すはず)。
        let (second_suffix, second_guard) =
            resolve_generation_suffix(&files, dir.path(), 100).unwrap();
        assert_eq!(
            second_suffix, "_1",
            "1回目がロックを保持している間は同じsuffixを使わせてはいけない"
        );

        drop(first_guard);
        let (third_suffix, _third_guard) =
            resolve_generation_suffix(&files, dir.path(), 100).unwrap();
        assert_eq!(third_suffix, "", "ロック解放後は再び\"\"が使えるべき");

        drop(second_guard);
    }

    /// 強制終了（Ctrl+C、タスクマネージャ、GUI の × 終了、電源断）で
    /// ロックファイルだけが残ると、以降ずっと `_1`,`_2`... へフォールバックし続けて
    /// 出力ファイル名が永久にずれる。持ち主のいないロックは奪い返せること。
    #[cfg(windows)]
    #[test]
    fn resolve_suffix_reclaims_stale_lock_left_by_killed_process() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![dir.path().join("a.wav"), dir.path().join("b.srt")];
        let stale = dir.path().join(".s2v_generation.lock");
        std::fs::write(&stale, b"").unwrap(); // 誰もハンドルを持っていない残骸

        let (suffix, guard) = resolve_generation_suffix(&files, dir.path(), 100).unwrap();
        assert_eq!(suffix, "", "残骸ロックは奪い返して既定名で出力すべき");

        drop(guard);
        assert!(!stale.exists(), "奪い返したロックは解放時に削除されるべき");
    }

    /// 残骸の奪い返しが、生存プロセスのロックまで奪ってしまわないこと。
    /// （`resolve_suffix_atomically_claims_slot_preventing_concurrent_reuse` の
    ///   保証が残骸判定の導入で壊れていないかを、残骸ファイル混在下で確認する）
    #[cfg(windows)]
    #[test]
    fn resolve_suffix_does_not_steal_a_held_lock() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![dir.path().join("a.wav")];

        let (_first, first_guard) = resolve_generation_suffix(&files, dir.path(), 100).unwrap();
        // _1 のスロットには残骸ロックを置いておく（こちらは奪えるべき）
        std::fs::write(dir.path().join(".s2v_generation_1.lock"), b"").unwrap();

        let (second, second_guard) = resolve_generation_suffix(&files, dir.path(), 100).unwrap();
        assert_eq!(
            second, "_1",
            "保持中のロックは奪わず、残骸のある _1 を使うべき"
        );

        drop(second_guard);
        drop(first_guard);
    }

    #[test]
    fn generate_outputs_with_suffix_writes_numbered_names() {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path();
        let wav = out_dir.join("voice_0001_2.wav");
        write_wav(&wav, 48000, 0.1);
        let events = vec![make_audio_event(0.0, 100.0, "テスト", Some(wav.clone()))];
        let exp = Exporter::new(&events, out_dir, 48000, default_bgm());
        exp.generate_srt("_2").unwrap();
        exp.generate_fcpxml("_2").unwrap();
        exp.generate_combined_audio("_2").unwrap();
        assert!(out_dir.join("timeline/subtitles_2.srt").exists());
        assert!(out_dir.join("timeline/timeline_2.fcpxml").exists());
        assert!(out_dir.join("full_dialogue_2.wav").exists());
    }

    #[test]
    fn fcpxml_references_suffixed_voice_path() {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path();
        let wav = out_dir.join("voice_0001_2.wav");
        write_wav(&wav, 48000, 0.1);
        let events = vec![make_audio_event(0.0, 100.0, "テスト", Some(wav.clone()))];
        let exp = Exporter::new(&events, out_dir, 48000, default_bgm());
        exp.generate_fcpxml("_2").unwrap();
        let xml = std::fs::read_to_string(out_dir.join("timeline/timeline_2.fcpxml")).unwrap();
        assert!(
            xml.contains("voice_0001_2.wav"),
            "FCPXMLは連番付き音声を参照すること: {xml}"
        );
    }

    /// `C:\work\A&B\voice.wav` のようなパスをそのままXML属性へ埋め込むと不正なXMLになる
    /// (review.txt指摘)。`<`/`>`/`"` はWindowsのファイル名に使えないため、
    /// 実際に発生し得る `&` を含むディレクトリ名・ファイル名で再現する。
    #[test]
    fn fcpxml_escapes_ampersand_in_asset_name_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let sub_dir = dir.path().join("A&B");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let wav = sub_dir.join("voice&1.wav");
        write_wav(&wav, 48000, 0.1);

        let events = vec![make_audio_event(0.0, 100.0, "テスト", Some(wav))];
        let exp = Exporter::new(&events, dir.path(), 48000, default_bgm());
        exp.generate_fcpxml("").unwrap();
        let xml = std::fs::read_to_string(dir.path().join("timeline/timeline.fcpxml")).unwrap();

        // 生のまま(未エスケープ)の "&1.wav" のような不正な並びが出てはいけない
        assert!(
            !xml.contains("voice&1.wav"),
            "&はエスケープされているべき: {xml}"
        );
        assert!(
            xml.contains("voice&amp;1.wav"),
            "ファイル名の&はエスケープされているべき: {xml}"
        );
        assert!(
            xml.contains("A&amp;B"),
            "ディレクトリ名の&もエスケープされているべき: {xml}"
        );
    }

    #[test]
    fn xml_escape_handles_all_reserved_characters() {
        assert_eq!(xml_escape(r#"A&B<C>"D""#), "A&amp;B&lt;C&gt;&quot;D&quot;");
        assert_eq!(xml_escape("plain"), "plain");
    }

    #[test]
    fn timeline_json_has_relative_paths_and_names() {
        let dir = tempfile::tempdir().unwrap();
        let audio_path = dir.path().join("audio").join("voice_0001.wav");
        let events = vec![
            TimelineEvent {
                event_type: EventType::Audio,
                start_ms: 0.0,
                duration_ms: 1500.0,
                path: Some(audio_path),
                text: Some("とうきょう".to_string()),
                display_text: Some("東京".to_string()),
                cast: Some("A".to_string()),
                name: None,
            },
            make_named_paragraph_event(1500.0, "オープニング"),
        ];
        Exporter::new(&events, dir.path(), 48000, default_bgm())
            .generate_timeline_json("")
            .unwrap();

        let text = std::fs::read_to_string(dir.path().join("timeline/timeline.json")).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["version"], 1);
        assert_eq!(doc["sample_rate"], 48000);
        assert_eq!(doc["total_ms"], 1500.0);
        assert_eq!(doc["events"][0]["path"], "audio/voice_0001.wav");
        assert_eq!(doc["events"][0]["cast"], "A");
        assert_eq!(doc["events"][1]["event_type"], "paragraph");
        assert_eq!(doc["events"][1]["name"], "オープニング");
    }

    #[test]
    fn timeline_json_respects_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let events = vec![make_paragraph_event(100.0)];
        Exporter::new(&events, dir.path(), 48000, default_bgm())
            .generate_timeline_json("_2")
            .unwrap();
        assert!(dir.path().join("timeline/timeline_2.json").exists());
    }

    // ---- s2v-18b.5: FCPXML のファイル URI ----

    #[test]
    fn file_uri_strips_windows_verbatim_prefix() {
        assert_eq!(
            file_uri(Path::new(r"\\?\D:\UDS\out\audio\voice_0001.wav")),
            "file:///D:/UDS/out/audio/voice_0001.wav"
        );
        assert_eq!(
            file_uri(Path::new(r"\\?\UNC\server\share\a.wav")),
            "file://server/share/a.wav"
        );
        assert_eq!(file_uri(Path::new(r"D:\a\b.wav")), "file:///D:/a/b.wav");
    }

    #[test]
    fn fcpxml_asset_src_is_a_valid_file_uri() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("voice_0001.wav");
        write_wav(&wav, 48000, 0.1);
        let events = vec![make_audio_event(0.0, 100.0, "テスト", Some(wav))];
        Exporter::new(&events, dir.path(), 48000, default_bgm())
            .generate_fcpxml("")
            .unwrap();
        let content = std::fs::read_to_string(dir.path().join("timeline/timeline.fcpxml")).unwrap();
        assert!(content.contains(r#"src="file:///"#), "{content}");
        assert!(
            !content.contains("?/"),
            "verbatim 接頭辞が残ってはいけない: {content}"
        );
    }

    // ---- s2v-18b.6: BGM/SE のサンプルレート変換 ----

    /// 44.1kHz の 1kHz 正弦波を 48kHz で読み込んだとき、理想的な 48kHz 正弦波との誤差が小さいこと。
    /// 最近傍法（以前の実装）だと誤差 RMS が 0.05 前後になる。
    #[test]
    fn read_stereo_float_resamples_with_band_limited_interpolation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bgm44k.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44100,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        let f = 1000.0_f64;
        for i in 0..44100 {
            let v = (2.0 * std::f64::consts::PI * f * i as f64 / 44100.0).sin() as f32 * 0.5;
            w.write_sample(v).unwrap();
            w.write_sample(-v).unwrap();
        }
        w.finalize().unwrap();

        let out = read_stereo_float(&path, 48000).unwrap();
        assert_eq!(out.len(), 48000);
        // 端の過渡応答を除いた中央部で比較する
        let (mut err_l, mut err_r, mut n) = (0.0_f64, 0.0_f64, 0usize);
        for (i, s) in out.iter().enumerate().take(44000).skip(4000) {
            let ideal = (2.0 * std::f64::consts::PI * f * i as f64 / 48000.0).sin() * 0.5;
            err_l += (s[0] as f64 - ideal).powi(2);
            err_r += (s[1] as f64 + ideal).powi(2);
            n += 1;
        }
        let rms_l = (err_l / n as f64).sqrt();
        let rms_r = (err_r / n as f64).sqrt();
        assert!(
            rms_l < 0.005 && rms_r < 0.005,
            "rms_l={rms_l} rms_r={rms_r}"
        );
    }
}
