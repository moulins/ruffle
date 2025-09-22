//! `Boolean` class impl

use ruffle_macros::istr;

use crate::avm1::activation::Activation;
use crate::avm1::error::Error;
use crate::avm1::property_decl::{DeclContext, Declaration, SystemClass};
use crate::avm1::{NativeObject, Object, Value};

const PROTO_DECLS: &[Declaration] = declare_properties! {
    "toString" => method(to_string; DONT_ENUM | DONT_DELETE);
    "valueOf" => method(value_of; DONT_ENUM | DONT_DELETE);
};

pub fn create_class<'gc>(
    context: &mut DeclContext<'_, 'gc>,
    super_proto: Object<'gc>,
) -> SystemClass<'gc> {
    let class = context.builtin_class(constructor, super_proto);
    context.define_properties_on(class.proto, PROTO_DECLS);
    class
}

pub fn populate_this<'gc>(activation: &mut Activation<'_, 'gc>, this: Object<'gc>, value: bool) {
    this.set_native(activation.gc(), NativeObject::Bool(value));
}

/// Implements `Boolean` constructor and function
fn constructor<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Object<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let is_constructor = activation.consume_native_constructor_flag();

    let value = args
        .get(0)
        .map(|value| value.as_bool(activation.swf_version()));

    if is_constructor {
        // Called from a constructor, populate `this`.
        populate_this(activation, this, value.unwrap_or(false));
        Ok(this.into())
    } else {
        // If Boolean is called as a function, return the value.
        // Boolean() with no argument returns undefined.
        Ok(value.map(Value::from).unwrap_or(Value::Undefined))
    }
}

pub fn to_string<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    // Must be a bool.
    // Boolean.prototype.toString.call(x) returns undefined for non-bools.
    if let NativeObject::Bool(value) = this.native() {
        return Ok(Value::from(match value {
            true => istr!("true"),
            false => istr!("false"),
        }));
    }

    Ok(Value::Undefined)
}

pub fn value_of<'gc>(
    _activation: &mut Activation<'_, 'gc>,
    this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    // Must be a bool.
    // Boolean.prototype.valueOf.call(x) returns undefined for non-bools.
    if let NativeObject::Bool(value) = this.native() {
        return Ok(value.into());
    }

    Ok(Value::Undefined)
}
