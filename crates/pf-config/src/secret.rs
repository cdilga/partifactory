//! `Secret<T>`: a value that never prints, never serializes and is wiped on drop.

use std::fmt;
use zeroize::{Zeroize, Zeroizing};

/// A secret value. `Debug` and `Display` print `[redacted]`; there is deliberately no
/// `Serialize`; the inner value is zeroized when dropped. Read it with [`Secret::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T: Zeroize>(Zeroizing<T>);

impl<T: Zeroize> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(Zeroizing::new(value))
    }

    /// The only way to read the value. Grep for `expose()` to audit every use.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}
