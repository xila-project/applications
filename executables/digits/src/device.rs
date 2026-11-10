//! Typed use of the existing generic WASI descriptor-control extension.
use device::neural_network::*;
use file_system::ControlCommand;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
};

#[repr(align(16))]
struct Aligned<T>(T);

fn control<C: ControlCommand>(file: &File, _: C, value: C::Input) -> io::Result<C::Output>
where
    C::Output: Default,
{
    let input = Aligned(value);
    let mut output = Aligned(C::Output::default());
    // SAFETY: disjoint live input/output, both aligned to 16 bytes. Each command
    // uses its Core-defined payload layout and identifier, never guest pointers.
    let result = unsafe {
        wasm::fd_control(
            file.as_raw_fd(),
            C::IDENTIFIER.raw(),
            core::ptr::from_ref(&input.0).cast(),
            size_of::<C::Input>() as u32,
            core::ptr::from_mut(&mut output.0).cast(),
            size_of::<C::Output>() as u32,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    Ok(output.0)
}

pub struct Classifier(File);

impl Classifier {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(digits::DEVICE_PATH)?;
        let input = control(&file, GET_INPUT_INFO, 0)?;
        let output = control(&file, GET_OUTPUT_INFO, 0)?;
        if control(&file, GET_INPUT_COUNT, ())? != 1
            || control(&file, GET_OUTPUT_COUNT, ())? != 1
            || !matches_tensor(input, &[1, 784], 784 * 4)
            || !matches_tensor(output, &[1, 10], 10 * 4)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incompatible digit model",
            ));
        }
        Ok(Self(file))
    }

    pub fn predict(&mut self, pixels: &[f32; digits::INPUT_LEN]) -> io::Result<[(u8, f32); 3]> {
        control(&self.0, SET_INPUT_INDEX, 0)?;
        self.0.seek(SeekFrom::Start(0))?;
        let mut bytes = [0u8; digits::INPUT_LEN * 4];
        for (value, chunk) in pixels.iter().zip(bytes.as_chunks_mut::<4>().0) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        self.0.write_all(&bytes)?;
        control(&self.0, COMPUTE, ())?;
        control(&self.0, SET_OUTPUT_INDEX, 0)?;
        self.0.seek(SeekFrom::Start(0))?;
        let mut bytes = [0u8; 40];
        self.0.read_exact(&mut bytes)?;
        let scores = core::array::from_fn(|i| {
            f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
        });
        digits::top_three(scores).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid digit probabilities")
        })
    }
}

fn matches_tensor(info: TensorInfo, shape: &[i32], bytes: u32) -> bool {
    info.data_type == 0 // TFLite FLOAT32
        && info.rank as usize == shape.len()
        && info.dimensions[..shape.len()] == *shape
        && info.byte_len == bytes
        && info.quantization_count == 0
}
