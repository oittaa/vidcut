use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub size: u32,
    pub range: [f32; 2],
    pub rows: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cube {
    pub one: Option<Table>,
    pub three: Option<Table>,
}

const USED_KEYWORDS: [&str; 4] = [
    "LUT_1D_SIZE",
    "LUT_3D_SIZE",
    "LUT_1D_INPUT_RANGE",
    "LUT_3D_INPUT_RANGE",
];

const BYTES_IN_A_ROW: usize = 32;

fn number(word: Option<&str>) -> Option<f32> {
    word?.parse().ok().filter(|v: &f32| v.is_finite())
}

fn size_of(key: &str, word: Option<&str>, line: usize) -> Result<u32, String> {
    let Some(size) = word.and_then(|w| w.parse::<u32>().ok()) else {
        return Err(format!("line {line}: {key} needs a whole number"));
    };
    if size < 2 {
        return Err(format!("line {line}: {key} {size}: the least size is 2"));
    }
    Ok(size)
}

fn range_of(key: &str, a: Option<&str>, b: Option<&str>, line: usize) -> Result<[f32; 2], String> {
    let (Some(low), Some(high)) = (number(a), number(b)) else {
        return Err(format!("line {line}: {key} needs two numbers"));
    };
    if low == high {
        return Err(format!("line {line}: {key} {low} {high} has no width"));
    }
    Ok([low, high])
}

pub fn parse(bytes: &[u8]) -> Result<Cube, String> {
    let mut sizes = [None, None];
    let mut ranges = [[0.0, 1.0]; 2];
    let mut rows: Vec<[f32; 3]> = Vec::with_capacity(bytes.len() / BYTES_IN_A_ROW);
    for (i, whole) in bytes.split(|b| *b == b'\n').enumerate() {
        let line = i + 1;
        for part in whole.split(|b| *b == b'\r') {
            let text = String::from_utf8_lossy(part);
            let mut words = text.trim_start_matches('\u{feff}').split_whitespace();
            let Some(first) = words.next() else {
                continue;
            };
            if first.starts_with('#') {
                continue;
            }
            if let Ok(r) = first.parse::<f32>() {
                let (g, b) = (number(words.next()), number(words.next()));
                let (true, Some(g), Some(b)) = (r.is_finite(), g, b) else {
                    return Err(format!("line {line}: a row needs three finite numbers"));
                };
                rows.push([r, g, b]);
                continue;
            }
            match first {
                "LUT_1D_SIZE" => sizes[0] = Some(size_of(first, words.next(), line)?),
                "LUT_3D_SIZE" => sizes[1] = Some(size_of(first, words.next(), line)?),
                "LUT_1D_INPUT_RANGE" => {
                    ranges[0] = range_of(first, words.next(), words.next(), line)?
                }
                "LUT_3D_INPUT_RANGE" => {
                    ranges[1] = range_of(first, words.next(), words.next(), line)?
                }
                other if USED_KEYWORDS.contains(&other.to_ascii_uppercase().as_str()) => {
                    return Err(format!(
                        "line {line}: {other} must be in upper case, as {}",
                        other.to_ascii_uppercase()
                    ));
                }
                _ => {}
            }
        }
    }
    let [one, three] = sizes;
    if one.is_none() && three.is_none() {
        return Err("it has no LUT_1D_SIZE or LUT_3D_SIZE line".into());
    }
    let first = one.map_or(0, u128::from);
    let due = first + three.map_or(0, |n| u128::from(n).pow(3));
    if (rows.len() as u128) < due {
        return Err(format!("it has {} rows, and {due} are due", rows.len()));
    }
    rows.truncate(due as usize);
    let rest = rows.split_off(first as usize);
    let table = |size: u32, range: [f32; 2], rows: Vec<[f32; 3]>| Table { size, range, rows };
    Ok(Cube {
        one: one.map(|n| table(n, ranges[0], rows)),
        three: three.map(|n| table(n, ranges[1], rest)),
    })
}

pub fn read(path: &Path, bytes: &[u8]) -> Result<Cube, String> {
    parse(bytes).map_err(|why| format!("{}: {why}", path.display()))
}

pub fn stamp(path: &Path) -> String {
    let (size, time) = crate::store::stamp_of(path).unzip();
    format!("{}|{size:?}|{time:?}", path.display())
}
