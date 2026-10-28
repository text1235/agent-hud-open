//! Original Antigravity 2.19.1 embedded descriptor fields. Adapted from the
//! MIT CodexBar reader credited in the original project's third-party notices.
use crate::protobuf::{fields, Field};

#[derive(Default, Clone, PartialEq)]
pub struct Usage {
    pub model: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub read: u64,
    pub write: u64,
    pub thinking: u64,
    pub response: u64,
    pub response_id: Option<String>,
    pub message: Option<String>,
    invalid_message: bool,
}

#[derive(Default, Clone, PartialEq)]
pub struct Turn {
    pub usage: Option<Usage>,
    pub at: Option<i64>,
    pub model: Option<String>,
    pub label: Option<String>,
    pub requested: Option<u64>,
    pub execution: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct TrajectoryMetadata {
    pub cwd: Option<String>,
    pub parent_id: Option<String>,
    pub root_id: Option<String>,
    pub agent_name: Option<String>,
    pub role: Option<String>,
    pub created_at: Option<i64>,
}

#[derive(Default)]
struct Timestamp {
    seconds: Option<u64>,
    nanos: u64,
}

impl Timestamp {
    fn read(&mut self, b: &[u8]) -> Result<(), ()> {
        fields(b, |f| {
            match f.number {
                1 => {
                    let n = f.integer()?;
                    if n == 0 || n > 253_402_300_799 {
                        return Err(());
                    }
                    self.seconds = Some(n)
                }
                2 => {
                    let n = f.integer()?;
                    if n > 999_999_999 {
                        return Err(());
                    }
                    self.nanos = n
                }
                _ => (),
            }
            Ok(())
        })
    }
    fn ms(&self) -> Option<i64> {
        self.seconds
            .map(|s| (s * 1000 + self.nanos / 1_000_000) as i64)
    }
}

fn auxiliary(f: Field<'_>, value: &mut Option<String>, invalid: &mut bool) {
    match f.string() {
        Ok(s) if !*invalid => *value = s,
        Err(()) => {
            *value = None;
            *invalid = true
        }
        _ => (),
    }
}

pub fn turn(bytes: &[u8]) -> Option<Turn> {
    let mut t = Turn::default();
    let mut time = Timestamp::default();
    let mut streaming_ms: u64 = 0;
    let mut ttft_ms: u64 = 0;
    let mut found = false;
    fields(bytes, |f| {
        match f.number {
            1 => {
                found = true;
                fields(f.bytes()?, |c| {
                    match c.number {
                        3 => t.requested = Some(c.counter()?),
                        4 => {
                            let u = t.usage.get_or_insert_with(Usage::default);
                            fields(c.bytes()?, |v| {
                                match v.number {
                                    1 => u.model = Some(v.counter()?),
                                    2 => u.input = v.counter()?,
                                    3 => u.output = v.counter()?,
                                    4 => u.write = v.counter()?,
                                    5 => u.read = v.counter()?,
                                    7 => auxiliary(v, &mut u.message, &mut u.invalid_message),
                                    9 => u.thinking = v.counter()?,
                                    10 => u.response = v.counter()?,
                                    11 => u.response_id = v.string()?,
                                    _ => (),
                                }
                                Ok(())
                            })?
                        }
                        9 => fields(c.bytes()?, |s| {
                            if s.number == 4 {
                                time.read(s.bytes()?)?
                            }
                            Ok(())
                        })?,
                        11 => streaming_ms = duration_ms(c.bytes()?).unwrap_or(0),
                        12 => ttft_ms = duration_ms(c.bytes()?).unwrap_or(0),
                        19 => t.model = c.string()?,
                        21 => t.label = c.string()?,
                        _ => (),
                    }
                    Ok(())
                })?
            }
            4 => t.execution = f.string()?,
            _ => (),
        }
        Ok(())
    })
    .ok()?;
    t.at = time.ms();
    let total_ms = streaming_ms.saturating_add(ttft_ms);
    if total_ms > 0 {
        t.duration_ms = Some(total_ms);
    }
    found.then_some(t)
}

fn duration_ms(bytes: &[u8]) -> Option<u64> {
    let mut seconds: u64 = 0;
    let mut nanos: u64 = 0;
    fields(bytes, |f| {
        match f.number {
            1 => seconds = f.counter()?,
            2 => nanos = f.counter()?,
            _ => (),
        }
        Ok(())
    })
    .ok()?;
    let ms = seconds
        .saturating_mul(1000)
        .saturating_add(nanos / 1_000_000);
    (ms > 0 && ms <= 86_400_000).then_some(ms)
}

fn file_uri_to_path(raw: Option<String>) -> Option<String> {
    let text = raw?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("file://") {
        url::Url::parse(trimmed)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .and_then(|p| p.to_str().map(str::to_owned))
            .or_else(|| (!rest.is_empty()).then(|| rest.to_owned()))
    } else if trimmed.starts_with('/') {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

pub fn metadata(bytes: &[u8]) -> Option<TrajectoryMetadata> {
    let mut meta = TrajectoryMetadata::default();
    let mut time = Timestamp::default();
    let mut valid_time = false;
    fields(bytes, |f| {
        match f.number {
            1 => {
                let _ = fields(f.bytes()?, |w| {
                    if matches!(w.number, 1 | 2) && meta.cwd.is_none() {
                        meta.cwd = file_uri_to_path(w.string()?);
                    }
                    Ok(())
                });
            }
            2 => {
                if f.bytes().and_then(|b| time.read(b)).is_ok() {
                    valid_time = true;
                }
            }
            4 => {
                let _ = fields(f.bytes()?, |s| {
                    if s.number == 1 && meta.agent_name.is_none() {
                        meta.agent_name = s.string()?;
                    }
                    Ok(())
                });
            }
            5 => meta.parent_id = f.string()?.filter(|s| !s.is_empty()),
            6 => meta.root_id = f.string()?.filter(|s| !s.is_empty()),
            7 => {
                if meta.cwd.is_none() {
                    meta.cwd = file_uri_to_path(f.string()?);
                }
            }
            8 => {
                let _ = fields(f.bytes()?, |s| {
                    match s.number {
                        1 if meta.agent_name.is_none() => meta.agent_name = s.string()?,
                        2 => meta.role = s.string()?.filter(|r| !r.trim().is_empty()),
                        _ => (),
                    }
                    Ok(())
                });
            }
            _ => (),
        }
        Ok(())
    })
    .ok()?;
    if valid_time {
        meta.created_at = time.ms();
    }
    (meta.cwd.is_some()
        || meta.parent_id.is_some()
        || meta.root_id.is_some()
        || meta.agent_name.is_some()
        || meta.role.is_some()
        || meta.created_at.is_some())
    .then_some(meta)
}

#[derive(Default)]
pub struct Step {
    pub execution: Option<String>,
    pub message: Option<String>,
    pub at: Option<i64>,
}

pub fn step(bytes: &[u8]) -> Option<Step> {
    let mut s = Step::default();
    let mut time = Timestamp::default();
    let mut invalid = false;
    let mut valid_time = true;
    fields(bytes, |f| {
        match f.number {
            1 => {
                if f.bytes().and_then(|b| time.read(b)).is_err() {
                    valid_time = false
                }
            }
            9 => {
                if f.bytes()
                    .and_then(|b| {
                        fields(b, |f| {
                            if f.number == 7 {
                                auxiliary(f, &mut s.message, &mut invalid)
                            }
                            Ok(())
                        })
                    })
                    .is_err()
                {
                    s.message = None;
                    invalid = true
                }
            }
            12 => s.execution = f.string()?,
            _ => (),
        }
        Ok(())
    })
    .ok()?;
    s.at = valid_time.then(|| time.ms()).flatten();
    Some(s)
}
