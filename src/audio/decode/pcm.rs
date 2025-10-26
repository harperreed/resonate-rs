// ABOUTME: PCM decoder implementation
// ABOUTME: Supports 16-bit and 24-bit PCM decoding with zero-copy where possible

use crate::audio::Sample;
use crate::audio::decode::Decoder;
use crate::error::Error;
use std::sync::Arc;

pub struct PcmDecoder {
    bit_depth: u8,
}

impl PcmDecoder {
    pub fn new(bit_depth: u8) -> Self {
        Self { bit_depth }
    }
}

impl Decoder for PcmDecoder {
    fn decode(&self, data: &[u8]) -> Result<Arc<[Sample]>, Error> {
        match self.bit_depth {
            16 => {
                // Convert 16-bit PCM to Sample
                let samples: Vec<Sample> = data
                    .chunks_exact(2)
                    .map(|c| {
                        let i16_val = i16::from_le_bytes([c[0], c[1]]);
                        Sample::from_i16(i16_val)
                    })
                    .collect();
                Ok(Arc::from(samples.into_boxed_slice()))
            }
            24 => {
                // Convert 24-bit PCM to Sample
                let samples: Vec<Sample> = data
                    .chunks_exact(3)
                    .map(|c| Sample::from_i24_le([c[0], c[1], c[2]]))
                    .collect();
                Ok(Arc::from(samples.into_boxed_slice()))
            }
            _ => Err(Error::Protocol(format!("Unsupported bit depth: {}", self.bit_depth))),
        }
    }
}
