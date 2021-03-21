//! Type-safe units for buffers.
//!
//! It is often ambiguous what an index or offset means when dealing with text. This module
//! provides types that include a strongly-typed unit to make the use of byte and codepoint
//! indices more explicit.

use euclid::{Length, Point2D};

#[derive(Debug)]
pub struct ByteSpace;

/// 1-dimensional index of a byte within a buffer.
pub type ByteIndex = Length<usize, ByteSpace>;

/// 2-dimensional Position of a byte in the buffer.
///
/// `y` is the line number, `x` is the byte index within the line.
pub type BytePosition = Point2D<usize, ByteSpace>;

#[derive(Debug)]
pub struct CharacterSpace;

/// 2-dimensional position of a UTF-8 codepoint in the buffer.
///
/// `y` is the line number, `x` is the character index within the line.
pub type CharPosition = Point2D<usize, CharacterSpace>;
