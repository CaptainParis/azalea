//! What the fuzz targets run, shared with the seed tests (`tests/`) so a seed
//! is checked by exactly the code that fuzzes it.
//!
//! Every target treats a panic as a finding. Decode errors are fine (that's
//! most fuzz inputs); a value that decodes must survive whatever a bot does
//! with it (encode, format, convert, hand to the client).

pub mod input;
pub mod seeds;
pub mod targets;
