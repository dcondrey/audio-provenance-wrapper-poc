//! A pass-through `Serializer` that refuses non-finite floats.
//!
//! IMPORTANT: `serde_json`'s own `serialize_f64` writes `null` for NaN and the
//! infinities *before* the `Formatter` is consulted, so neither a custom
//! formatter nor `to_value` can see one. Without this wrapper a NaN crest
//! factor would be laundered into a signed manifest as `null`, which Python's
//! `allow_nan=False` would have refused to produce.

use serde::ser::{
    Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant, Serializer,
};

pub(crate) const NON_FINITE_MARKER: &str = "apw-core rejects a non-finite number";

pub(crate) struct Guarded<'a, T: ?Sized>(pub &'a T);

impl<T: Serialize + ?Sized> Serialize for Guarded<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(Guard(serializer))
    }
}

pub(crate) struct Guard<S>(pub S);

fn non_finite<E: serde::ser::Error>() -> E {
    E::custom(NON_FINITE_MARKER)
}

macro_rules! forward_scalar {
    ($($method:ident($ty:ty),)*) => {
        $(
            fn $method(self, value: $ty) -> Result<Self::Ok, Self::Error> {
                self.0.$method(value)
            }
        )*
    };
}

impl<S: Serializer> Serializer for Guard<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = GuardCompound<S::SerializeSeq>;
    type SerializeTuple = GuardCompound<S::SerializeTuple>;
    type SerializeTupleStruct = GuardCompound<S::SerializeTupleStruct>;
    type SerializeTupleVariant = GuardCompound<S::SerializeTupleVariant>;
    type SerializeMap = GuardCompound<S::SerializeMap>;
    type SerializeStruct = GuardCompound<S::SerializeStruct>;
    type SerializeStructVariant = GuardCompound<S::SerializeStructVariant>;

    forward_scalar! {
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
        serialize_unit_struct(&'static str),
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        if value.is_finite() {
            self.0.serialize_f32(value)
        } else {
            Err(non_finite())
        }
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        if value.is_finite() {
            self.0.serialize_f64(value)
        } else {
            Err(non_finite())
        }
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_none()
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_some(&Guarded(value))
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_variant(name, index, variant)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_newtype_struct(name, &Guarded(value))
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0
            .serialize_newtype_variant(name, index, variant, &Guarded(value))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.0.serialize_seq(len).map(GuardCompound)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.0.serialize_tuple(len).map(GuardCompound)
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.0.serialize_tuple_struct(name, len).map(GuardCompound)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.0
            .serialize_tuple_variant(name, index, variant, len)
            .map(GuardCompound)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.0.serialize_map(len).map(GuardCompound)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.0.serialize_struct(name, len).map(GuardCompound)
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.0
            .serialize_struct_variant(name, index, variant, len)
            .map(GuardCompound)
    }

    fn collect_str<T: ?Sized + core::fmt::Display>(
        self,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.collect_str(value)
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

pub(crate) struct GuardCompound<C>(C);

impl<C: SerializeSeq> SerializeSeq for GuardCompound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_element(&Guarded(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<C: SerializeTuple> SerializeTuple for GuardCompound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_element(&Guarded(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<C: SerializeTupleStruct> SerializeTupleStruct for GuardCompound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(&Guarded(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<C: SerializeTupleVariant> SerializeTupleVariant for GuardCompound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(&Guarded(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<C: SerializeMap> SerializeMap for GuardCompound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.0.serialize_key(&Guarded(key))
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_value(&Guarded(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<C: SerializeStruct> SerializeStruct for GuardCompound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.0.serialize_field(key, &Guarded(value))
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        self.0.skip_field(key)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<C: SerializeStructVariant> SerializeStructVariant for GuardCompound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.0.serialize_field(key, &Guarded(value))
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        self.0.skip_field(key)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}
