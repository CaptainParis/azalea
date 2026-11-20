use std::{
    collections::HashMap,
    hash::Hash,
    io::{self, Cursor, Write},
    sync::Arc,
};

use indexmap::IndexMap;

use crate::{
    AzBuf, AzBufLimited, AzBufVar, BufReadError, MAX_STRING_LENGTH, UnsizedByteArray, limits,
    read_bytes, read_utf_with_len, write_utf_with_len,
};

impl AzBuf for UnsizedByteArray {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        // read to end of the buffer (nothing if the position is past it)
        let data = buf
            .get_ref()
            .get(buf.position() as usize..)
            .unwrap_or_default()
            .to_vec();
        buf.set_position((buf.position()) + data.len() as u64);
        Ok(UnsizedByteArray(data))
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        buf.write_all(self)
    }
}

macro_rules! impl_for_map_type {
    ($ty: ident) => {
        impl<K: AzBuf + Eq + Hash, V: AzBuf> AzBuf for $ty<K, V> {
            const MIN_ENCODED_SIZE: usize = 1;

            fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
                let min = K::MIN_ENCODED_SIZE + V::MIN_ENCODED_SIZE;
                let length = limits::read_collection_len(buf, min)?;
                limits::charge_memory::<(K, V)>(length)?;
                let mut contents =
                    Self::with_capacity(limits::initial_capacity::<(K, V)>(buf, length, min));
                for _ in 0..length {
                    limits::charge_element_if_zero_size(min)?;
                    contents.insert(K::azalea_read(buf)?, V::azalea_read(buf)?);
                }
                Ok(contents)
            }
            fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
                u32::azalea_write_var(&(self.len() as u32), buf)?;
                for (key, value) in self {
                    key.azalea_write(buf)?;
                    value.azalea_write(buf)?;
                }

                Ok(())
            }
        }
        impl<K: AzBuf + Eq + Hash, V: AzBufVar> AzBufVar for $ty<K, V> {
            fn azalea_read_var(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
                // a VarInt is at least one byte
                let min = K::MIN_ENCODED_SIZE + 1;
                let length = limits::read_collection_len(buf, min)?;
                limits::charge_memory::<(K, V)>(length)?;
                let mut contents =
                    Self::with_capacity(limits::initial_capacity::<(K, V)>(buf, length, min));
                for _ in 0..length {
                    contents.insert(K::azalea_read(buf)?, V::azalea_read_var(buf)?);
                }
                Ok(contents)
            }
            fn azalea_write_var(&self, buf: &mut impl Write) -> io::Result<()> {
                u32::azalea_write_var(&(self.len() as u32), buf)?;
                for (key, value) in self {
                    key.azalea_write(buf)?;
                    value.azalea_write_var(buf)?;
                }

                Ok(())
            }
        }
    };
}

impl_for_map_type!(HashMap);
impl_for_map_type!(IndexMap);

/// Read `length` elements of `T` into a `Vec`, preallocating at most what
/// [`limits::initial_capacity`] allows.
fn read_elements<T: AzBuf>(buf: &mut Cursor<&[u8]>, length: usize) -> Result<Vec<T>, BufReadError> {
    let min = T::MIN_ENCODED_SIZE;
    limits::charge_memory::<T>(length)?;
    let mut contents = Vec::with_capacity(limits::initial_capacity::<T>(buf, length, min));
    for _ in 0..length {
        limits::charge_element_if_zero_size(min)?;
        contents.push(T::azalea_read(buf)?);
    }
    Ok(contents)
}

macro_rules! impl_for_list_type {
    ($ty: ty) => {
        impl<T: AzBuf> AzBuf for $ty {
            const MIN_ENCODED_SIZE: usize = 1;

            default fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
                let length = limits::read_collection_len(buf, T::MIN_ENCODED_SIZE)?;
                Ok(read_elements::<T>(buf, length)?.into())
            }
            default fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
                (self.len() as u32).azalea_write_var(buf)?;
                for item in self.iter() {
                    T::azalea_write(item, buf)?;
                }
                Ok(())
            }
        }
        impl<T: AzBufVar> AzBufVar for $ty {
            fn azalea_read_var(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
                // a VarInt is at least one byte
                let length = limits::read_collection_len(buf, 1)?;
                limits::charge_memory::<T>(length)?;
                let mut contents =
                    Vec::with_capacity(limits::initial_capacity::<T>(buf, length, 1));
                for _ in 0..length {
                    contents.push(T::azalea_read_var(buf)?);
                }
                Ok(contents.into())
            }
            fn azalea_write_var(&self, buf: &mut impl Write) -> io::Result<()> {
                (self.len() as u32).azalea_write_var(buf)?;
                for item in self.iter() {
                    T::azalea_write_var(item, buf)?;
                }
                Ok(())
            }
        }
        impl<T: AzBuf> AzBufLimited for $ty {
            fn azalea_read_limited(
                buf: &mut Cursor<&[u8]>,
                limit: u32,
            ) -> Result<Self, BufReadError> {
                // ByteBufCodecs.readCount
                let length = limits::read_collection_len(buf, 0)?;
                if length > limit as usize {
                    return Err(limits::limit_error(
                        limits::LimitKind::CollectionLength,
                        length as u64,
                        limit as u64,
                    ));
                }
                limits::check_collection_len(buf, length, T::MIN_ENCODED_SIZE)?;
                Ok(read_elements::<T>(buf, length)?.into())
            }
        }
    };
}

impl_for_list_type!(Vec<T>);
impl_for_list_type!(Box<[T]>);
// `Arc<[T]>` is deliberately not implemented here, because converting a
// `Vec<T>` to `Arc<[T]>` results in allocations (since `Arc` stores the
// counters on the heap) and we'd like to avoid that. for us it's typically
// better to do `Arc<Box<[T]>>`.

impl AzBuf for Vec<u8> {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let length = limits::read_collection_len(buf, 1)?;
        read_bytes(buf, length).map(|b| b.to_vec())
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        (self.len() as u32).azalea_write_var(buf)?;
        buf.write_all(self)
    }
}

impl AzBuf for Box<[u8]> {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let length = limits::read_collection_len(buf, 1)?;
        read_bytes(buf, length).map(Box::from)
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        (self.len() as u32).azalea_write_var(buf)?;
        buf.write_all(self)
    }
}

impl AzBuf for String {
    const MIN_ENCODED_SIZE: usize = 1;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        read_utf_with_len(buf, MAX_STRING_LENGTH).map(Into::into)
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        write_utf_with_len(buf, self, MAX_STRING_LENGTH)
    }
}
impl AzBufLimited for String {
    fn azalea_read_limited(buf: &mut Cursor<&[u8]>, limit: u32) -> Result<Self, BufReadError> {
        read_utf_with_len(buf, limit).map(Into::into)
    }
}

impl AzBuf for Box<str> {
    const MIN_ENCODED_SIZE: usize = 1;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        read_utf_with_len(buf, MAX_STRING_LENGTH).map(Into::into)
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        write_utf_with_len(buf, self, MAX_STRING_LENGTH)
    }
}

impl AzBufLimited for Box<str> {
    fn azalea_read_limited(buf: &mut Cursor<&[u8]>, limit: u32) -> Result<Self, BufReadError> {
        String::azalea_read_limited(buf, limit).map(Into::into)
    }
}

impl<T: AzBuf> AzBuf for Option<T> {
    const MIN_ENCODED_SIZE: usize = 1;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let present = bool::azalea_read(buf)?;
        Ok(if present {
            Some(T::azalea_read(buf)?)
        } else {
            None
        })
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        if let Some(s) = self {
            true.azalea_write(buf)?;
            s.azalea_write(buf)?;
        } else {
            false.azalea_write(buf)?;
        };
        Ok(())
    }
}

impl<T: AzBufVar> AzBufVar for Option<T> {
    fn azalea_read_var(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let present = bool::azalea_read(buf)?;
        Ok(if present {
            Some(T::azalea_read_var(buf)?)
        } else {
            None
        })
    }
    fn azalea_write_var(&self, buf: &mut impl Write) -> io::Result<()> {
        if let Some(s) = self {
            true.azalea_write(buf)?;
            s.azalea_write_var(buf)?;
        } else {
            false.azalea_write(buf)?;
        };
        Ok(())
    }
}
impl<T: AzBufLimited> AzBufLimited for Option<T> {
    fn azalea_read_limited(buf: &mut Cursor<&[u8]>, limit: u32) -> Result<Self, BufReadError> {
        let present = bool::azalea_read(buf)?;
        Ok(if present {
            Some(T::azalea_read_limited(buf, limit)?)
        } else {
            None
        })
    }
}

impl<T: AzBuf, const N: usize> AzBuf for [T; N] {
    const MIN_ENCODED_SIZE: usize = T::MIN_ENCODED_SIZE * N;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let mut contents = Vec::with_capacity(N);
        for _ in 0..N {
            contents.push(T::azalea_read(buf)?);
        }
        Ok(contents
            .try_into()
            // network-safe: exactly N elements were pushed above (any read
            // error returned early)
            .unwrap_or_else(|_| unreachable!("The vec is the same size as the array")))
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        for i in self {
            i.azalea_write(buf)?;
        }
        Ok(())
    }
}

impl AzBuf for simdnbt::owned::NbtTag {
    const MIN_ENCODED_SIZE: usize = 1;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        crate::nbt::check_network_nbt(buf)?;
        Ok(simdnbt::owned::read_tag(buf).map_err(simdnbt::Error::from)?)
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        let mut data = Vec::new();
        self.write(&mut data);
        buf.write_all(&data)
    }
}

impl AzBuf for simdnbt::owned::NbtCompound {
    const MIN_ENCODED_SIZE: usize = 1;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        crate::nbt::check_network_nbt(buf)?;
        match simdnbt::owned::read_tag(buf).map_err(simdnbt::Error::from)? {
            simdnbt::owned::NbtTag::Compound(compound) => Ok(compound),
            _ => Err(BufReadError::Custom("Expected compound tag".to_owned())),
        }
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        let mut data = Vec::new();
        simdnbt::owned::NbtTag::Compound(self.clone()).write(&mut data);
        buf.write_all(&data)
    }
}

impl AzBuf for simdnbt::owned::Nbt {
    const MIN_ENCODED_SIZE: usize = 1;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        crate::nbt::check_network_nbt(buf)?;
        Ok(simdnbt::owned::read_unnamed(buf)?)
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        let mut data = Vec::new();
        self.write_unnamed(&mut data);
        buf.write_all(&data)
    }
}

impl<T> AzBuf for Box<T>
where
    T: AzBuf,
{
    const MIN_ENCODED_SIZE: usize = T::MIN_ENCODED_SIZE;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        T::azalea_read(buf).map(Box::new)
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        T::azalea_write(&**self, buf)
    }
}

impl<A: AzBuf, B: AzBuf> AzBuf for (A, B) {
    const MIN_ENCODED_SIZE: usize = A::MIN_ENCODED_SIZE + B::MIN_ENCODED_SIZE;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let a = A::azalea_read(buf)?;
        let b = B::azalea_read(buf)?;
        Ok((a, b))
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        self.0.azalea_write(buf)?;
        self.1.azalea_write(buf)
    }
}

impl<T: AzBuf> AzBuf for Arc<T> {
    const MIN_ENCODED_SIZE: usize = T::MIN_ENCODED_SIZE;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        Ok(Arc::new(T::azalea_read(buf)?))
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        T::azalea_write(&**self, buf)
    }
}
