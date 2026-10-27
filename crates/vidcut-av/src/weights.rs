use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

fn bad(path: &Path, title: &str, what: impl std::fmt::Display) -> Error {
    Error::Invalid(format!(
        "{}: not the weights of {title} ({what})",
        path.display()
    ))
}

struct Entry {
    dtype: String,
    shape: Vec<usize>,
    start: u64,
    end: u64,
}

pub(crate) struct Safetensors {
    path: PathBuf,
    title: &'static str,
    file: std::fs::File,
    base: u64,
    tensors: HashMap<String, Entry>,
}

impl Safetensors {
    pub(crate) fn open(path: &Path, title: &'static str) -> Result<Self> {
        let bad = |what: &str| bad(path, title, what);
        let mut file = std::fs::File::open(path)?;
        let mut len = [0u8; 8];
        file.read_exact(&mut len)?;
        let len = u64::from_le_bytes(len);
        if len > 100_000_000 {
            return Err(bad("header"));
        }
        let mut header = vec![0u8; len as usize];
        file.read_exact(&mut header)?;
        let json: serde_json::Value =
            serde_json::from_slice(&header).map_err(|e| bad(&e.to_string()))?;
        let map = json.as_object().ok_or_else(|| bad("header"))?;
        let mut tensors = HashMap::new();
        for (name, v) in map {
            if name == "__metadata__" {
                continue;
            }
            let dtype = v["dtype"].as_str().unwrap_or("").to_string();
            let shape = v["shape"]
                .as_array()
                .map(|a| a.iter().filter_map(|d| d.as_u64()).map(|d| d as usize))
                .ok_or_else(|| bad(name))?
                .collect();
            let offsets = v["data_offsets"].as_array().ok_or_else(|| bad(name))?;
            let (Some(start), Some(end)) = (
                offsets.first().and_then(|o| o.as_u64()),
                offsets.get(1).and_then(|o| o.as_u64()),
            ) else {
                return Err(bad(name));
            };
            tensors.insert(
                name.clone(),
                Entry {
                    dtype,
                    shape,
                    start,
                    end,
                },
            );
        }
        Ok(Self {
            path: path.to_path_buf(),
            title,
            file,
            base: 8 + len,
            tensors,
        })
    }

    pub(crate) fn floats(&mut self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let e = self
            .tensors
            .get(name)
            .ok_or_else(|| bad(&self.path, self.title, name))?;
        let count: usize = shape.iter().product();
        if e.dtype != "F32" || e.shape != shape || e.end - e.start != count as u64 * 4 {
            return Err(bad(&self.path, self.title, name));
        }
        self.file.seek(SeekFrom::Start(self.base + e.start))?;
        let mut raw = vec![0u8; count * 4];
        self.file.read_exact(&mut raw)?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dtype {
    F32,
    Bf16,
    F16,
}

impl Dtype {
    fn size(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::Bf16 | Self::F16 => 2,
        }
    }
}

#[derive(Debug, Clone)]
struct Tensor {
    storage: String,
    dtype: Dtype,
    offset: usize,
    shape: Vec<usize>,
    stride: Vec<usize>,
}

#[derive(Debug, Clone)]
enum Value {
    Mark,
    None,
    Int(i64),
    Str(String),
    Tuple(Vec<Value>),
    Global(String),
    Storage(String, Dtype),
    Tensor(Tensor),
    Dict(Vec<(Value, Value)>),
}

struct Pickle<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Pickle<'a> {
    fn take(&mut self, n: usize) -> std::result::Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or("the pickle ends early")?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn line(&mut self) -> std::result::Result<String, String> {
        let rest = &self.data[self.at..];
        let n = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("the pickle ends early")?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.at += n + 1;
        Ok(s)
    }

    fn u32(&mut self) -> std::result::Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

fn marked(stack: &mut Vec<Value>) -> std::result::Result<Vec<Value>, String> {
    let at = stack
        .iter()
        .rposition(|v| matches!(v, Value::Mark))
        .ok_or("a pickle tuple without its mark")?;
    let items = stack.split_off(at + 1);
    stack.pop();
    Ok(items)
}

fn ints(v: &Value) -> Option<Vec<usize>> {
    match v {
        Value::Tuple(items) => items
            .iter()
            .map(|i| match i {
                Value::Int(n) if *n >= 0 => Some(*n as usize),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn storage(pid: Value) -> std::result::Result<Value, String> {
    let Value::Tuple(items) = pid else {
        return Err("a storage id that is not a tuple".into());
    };
    match items.as_slice() {
        [Value::Str(kind), Value::Global(class), Value::Str(key), ..] if kind == "storage" => {
            let dtype = match class.as_str() {
                "torch.FloatStorage" => Dtype::F32,
                "torch.BFloat16Storage" => Dtype::Bf16,
                "torch.HalfStorage" => Dtype::F16,
                other => return Err(format!("storage of {other}")),
            };
            Ok(Value::Storage(key.clone(), dtype))
        }
        _ => Err("an unknown storage id".into()),
    }
}

fn call(callable: Value, args: Value) -> std::result::Result<Value, String> {
    let Value::Global(name) = callable else {
        return Err("a call of a value that is not a class".into());
    };
    let Value::Tuple(args) = args else {
        return Err("call arguments that are not a tuple".into());
    };
    match name.as_str() {
        "collections.OrderedDict" => Ok(Value::Dict(Vec::new())),
        "torch._utils._rebuild_tensor_v2" => match args.as_slice() {
            [
                Value::Storage(key, dtype),
                Value::Int(offset),
                shape,
                stride,
                ..,
            ] => Ok(Value::Tensor(Tensor {
                storage: key.clone(),
                dtype: *dtype,
                offset: usize::try_from(*offset).map_err(|e| e.to_string())?,
                shape: ints(shape).ok_or("a tensor shape")?,
                stride: ints(stride).ok_or("a tensor stride")?,
            })),
            _ => Err("tensor arguments".into()),
        },
        other => Err(format!("a call of {other}")),
    }
}

fn unpickle(data: &[u8]) -> std::result::Result<Value, String> {
    let mut p = Pickle { data, at: 0 };
    let mut stack: Vec<Value> = Vec::new();
    let mut memo: HashMap<u32, Value> = HashMap::new();
    let pop = |stack: &mut Vec<Value>| stack.pop().ok_or_else(|| "an empty stack".to_string());
    loop {
        let op = p.take(1)?[0];
        match op {
            0x80 => {
                p.take(1)?;
            }
            b'c' => {
                let module = p.line()?;
                let name = p.line()?;
                stack.push(Value::Global(format!("{module}.{name}")));
            }
            b'q' | b'r' => {
                let i = if op == b'q' {
                    u32::from(p.take(1)?[0])
                } else {
                    p.u32()?
                };
                let top = stack.last().ok_or("an empty stack")?.clone();
                memo.insert(i, top);
            }
            b'h' | b'j' => {
                let i = if op == b'h' {
                    u32::from(p.take(1)?[0])
                } else {
                    p.u32()?
                };
                stack.push(memo.get(&i).ok_or("a missing memo")?.clone());
            }
            b'(' => stack.push(Value::Mark),
            b')' => stack.push(Value::Tuple(Vec::new())),
            b't' => {
                let items = marked(&mut stack)?;
                stack.push(Value::Tuple(items));
            }
            0x85..=0x87 => {
                let n = usize::from(op - 0x84);
                if stack.len() < n {
                    return Err("an empty stack".into());
                }
                let items = stack.split_off(stack.len() - n);
                stack.push(Value::Tuple(items));
            }
            b'X' | 0x8c => {
                let n = if op == b'X' {
                    p.u32()? as usize
                } else {
                    usize::from(p.take(1)?[0])
                };
                let s = String::from_utf8_lossy(p.take(n)?).into_owned();
                stack.push(Value::Str(s));
            }
            b'J' => {
                let b = p.take(4)?;
                stack.push(Value::Int(i64::from(i32::from_le_bytes([
                    b[0], b[1], b[2], b[3],
                ]))));
            }
            b'K' => stack.push(Value::Int(i64::from(p.take(1)?[0]))),
            b'M' => {
                let b = p.take(2)?;
                stack.push(Value::Int(i64::from(u16::from_le_bytes([b[0], b[1]]))));
            }
            0x8a => {
                let n = usize::from(p.take(1)?[0]);
                let b = p.take(n)?;
                if n > 8 {
                    return Err("an integer too large".into());
                }
                let mut v: i64 = 0;
                for (i, byte) in b.iter().enumerate() {
                    v |= i64::from(*byte) << (8 * i);
                }
                if n > 0 && n < 8 && b[n - 1] & 0x80 != 0 {
                    v -= 1i64 << (8 * n);
                }
                stack.push(Value::Int(v));
            }
            0x88 => stack.push(Value::None),
            0x89 => stack.push(Value::None),
            b'N' => stack.push(Value::None),
            b'}' => stack.push(Value::Dict(Vec::new())),
            b'Q' => {
                let pid = pop(&mut stack)?;
                stack.push(storage(pid)?);
            }
            b'R' => {
                let args = pop(&mut stack)?;
                let callable = pop(&mut stack)?;
                stack.push(call(callable, args)?);
            }
            b'u' => {
                let items = marked(&mut stack)?;
                let Some(Value::Dict(entries)) = stack.last_mut() else {
                    return Err("items for a value that is not a dict".into());
                };
                let mut it = items.into_iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    entries.push((k, v));
                }
            }
            b's' => {
                let v = pop(&mut stack)?;
                let k = pop(&mut stack)?;
                let Some(Value::Dict(entries)) = stack.last_mut() else {
                    return Err("an item for a value that is not a dict".into());
                };
                entries.push((k, v));
            }
            b'b' => {
                pop(&mut stack)?;
            }
            b'.' => return pop(&mut stack),
            other => return Err(format!("pickle opcode {other:#04x}")),
        }
    }
}

pub(crate) struct Torch {
    path: PathBuf,
    title: &'static str,
    tensors: HashMap<String, Tensor>,
    storages: HashMap<String, Vec<u8>>,
}

impl Torch {
    pub(crate) fn open(path: &Path, title: &'static str) -> Result<Self> {
        let bad = |what: &dyn std::fmt::Display| bad(path, title, what);
        let file = std::fs::File::open(path)?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(&e))?;
        let pkl = zip
            .file_names()
            .filter_map(|n| n.ok())
            .find(|n| n.ends_with("data.pkl"))
            .map(|n| n.to_string())
            .ok_or_else(|| bad(&"no data.pkl"))?;
        let prefix = pkl.trim_end_matches("data.pkl").to_string();
        let mut data = Vec::new();
        zip.by_name(&pkl)
            .map_err(|e| bad(&e))?
            .read_to_end(&mut data)?;
        let value = unpickle(&data).map_err(|e| bad(&e))?;
        let tensors: HashMap<String, Tensor> = match value {
            Value::Tensor(t) => [(String::new(), t)].into(),
            Value::Dict(entries) => entries
                .into_iter()
                .filter_map(|(k, v)| match (k, v) {
                    (Value::Str(k), Value::Tensor(t)) => Some((k, t)),
                    _ => None,
                })
                .collect(),
            _ => return Err(bad(&"no tensors")),
        };
        let mut storages = HashMap::new();
        for t in tensors.values() {
            if storages.contains_key(&t.storage) {
                continue;
            }
            let mut bytes = Vec::new();
            zip.by_name(&format!("{prefix}data/{}", t.storage))
                .map_err(|e| bad(&e))?
                .read_to_end(&mut bytes)?;
            storages.insert(t.storage.clone(), bytes);
        }
        Ok(Self {
            path: path.to_path_buf(),
            title,
            tensors,
            storages,
        })
    }

    pub(crate) fn floats(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let t = self
            .tensors
            .get(name)
            .ok_or_else(|| bad(&self.path, self.title, name))?;
        let mut packed = vec![0usize; shape.len()];
        let mut step = 1;
        for (i, d) in shape.iter().enumerate().rev() {
            packed[i] = step;
            step *= d;
        }
        let packed_as = shape
            .iter()
            .zip(t.stride.iter().zip(&packed))
            .all(|(d, (s, p))| *d == 1 || s == p);
        if t.shape != shape || !packed_as {
            return Err(bad(&self.path, self.title, name));
        }
        let size = t.dtype.size();
        let bytes = &self.storages[&t.storage];
        let from = t.offset * size;
        let raw = bytes
            .get(from..from + step * size)
            .ok_or_else(|| bad(&self.path, self.title, name))?;
        Ok(match t.dtype {
            Dtype::F32 => raw
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
            Dtype::Bf16 => raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| f32::from_bits(u32::from(u16::from_le_bytes(*b)) << 16))
                .collect(),
            Dtype::F16 => raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| full(u16::from_le_bytes(*b)))
                .collect(),
        })
    }
}

fn full(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = u32::from((h >> 10) & 0x1f);
    let man = u32::from(h & 0x3ff);
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            let v = m as f32 / 1024.0 / 16384.0;
            return if sign != 0 { -v } else { v };
        }
        (0x1f, m) => sign | 0x7f80_0000 | (m << 13),
        (e, m) => sign | ((e + 112) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}
