//! Ombrac protocol implementation
//!
//! This crate provides the core protocol components:
//! - **codec**: Message encoding/decoding with length-delimited framing
//! - **protocol**: Protocol message definitions and serialization
//! - **reassembly**: UDP packet fragmentation and reassembly

pub mod codec;
pub mod protocol;
pub mod reassembly;
