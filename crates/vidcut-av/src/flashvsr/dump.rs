use std::io::Write;
use std::path::Path;

use crate::error::Result;

fn header(descr: &str, shape: &[usize]) -> Vec<u8> {
    let dims: Vec<String> = shape.iter().map(usize::to_string).collect();
    let dims = if dims.len() == 1 {
        format!("{},", dims[0])
    } else {
        dims.join(", ")
    };
    let mut text = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': ({dims}), }}");
    while (10 + text.len() + 1) % 64 != 0 {
        text.push(' ');
    }
    text.push('\n');
    let mut out = b"\x93NUMPY\x01\x00".to_vec();
    out.extend_from_slice(&(text.len() as u16).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

pub(super) fn write(path: &Path, shape: &[usize], data: &[f32]) -> Result<()> {
    let mut out = std::fs::File::create(path)?;
    out.write_all(&header("<f4", shape))?;
    out.write_all(&crate::compute::bytes(data))?;
    Ok(())
}

pub(super) fn write_bytes(path: &Path, shape: &[usize], data: &[u8]) -> Result<()> {
    let mut out = std::fs::File::create(path)?;
    out.write_all(&header("|u1", shape))?;
    out.write_all(data)?;
    Ok(())
}
