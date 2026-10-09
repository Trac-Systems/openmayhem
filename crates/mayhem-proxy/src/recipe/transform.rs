use super::{check, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
const MAX_DEPTH: usize = 24;
const MAX_NODES: usize = 65_536;
const MAX_PROGRAM: usize = 512;

/// Every present source field must have a mapping. There is no drop, constant,
/// default, formatting, string interpolation or conditional instruction operation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transform {
    Identity,
    Object {
        fields: BTreeMap<String, RequestField>,
    },
    Array {
        max_items: usize,
        item: Box<Transform>,
    },
    Enum {
        values: BTreeMap<String, String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestField {
    pub target: Vec<String>,
    pub transform: Transform,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Projection {
    Tuple {
        items: Vec<Projection>,
    },
    Copy {
        path: Vec<String>,
    },
    Literal {
        value: Value,
    },
    Object {
        fields: BTreeMap<String, Field>,
    },
    Array {
        path: Vec<String>,
        max_items: usize,
        item: Box<Projection>,
    },
    Enum {
        path: Vec<String>,
        values: BTreeMap<String, Value>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub optional: bool,
    pub value: Projection,
}
pub(super) fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.chars().any(char::is_control)
        && !matches!(value, "__proto__" | "prototype" | "constructor")
}
pub(super) fn path(value: &[String]) -> Result<()> {
    check(value.len() <= 16 && value.iter().all(|v| key(v)))
}
pub(super) fn read<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut value = root;
    for key in path {
        value = value.as_object()?.get(key)?;
    }
    Some(value)
}
fn scalar(value: &Value) -> bool {
    value.is_null()
        || value.is_boolean()
        || value.is_number()
        || value.as_str().is_some_and(|v| v.len() <= 1024)
}
struct Budget {
    nodes: usize,
    bytes: usize,
    max_bytes: usize,
}
impl Budget {
    fn step(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        check(depth <= MAX_DEPTH && self.nodes <= MAX_NODES)
    }
    fn bytes(&mut self, n: usize) -> Result<()> {
        self.bytes = self.bytes.checked_add(n).ok_or(Error)?;
        check(self.bytes <= self.max_bytes)
    }
    fn value(&mut self, v: &Value, depth: usize) -> Result<()> {
        self.step(depth)?;
        match v {
            Value::Array(a) => {
                check(a.len() <= 4096)?;
                self.bytes(2 + a.len())?;
                for v in a {
                    self.value(v, depth + 1)?;
                }
            }
            Value::Object(o) => {
                check(o.len() <= 512)?;
                self.bytes(2 + o.len())?;
                for (k, v) in o {
                    self.bytes(k.len() + 3)?;
                    self.value(v, depth + 1)?;
                }
            }
            Value::String(s) => self.bytes(s.len() + 2)?,
            _ => self.bytes(32)?,
        };
        Ok(())
    }
    fn clone_value(&mut self, v: &Value, depth: usize) -> Result<Value> {
        self.value(v, depth)?;
        Ok(v.clone())
    }
}
fn budget(max_bytes: usize) -> Budget {
    Budget {
        nodes: 0,
        bytes: 0,
        max_bytes,
    }
}
pub(super) fn bounded(value: &Value, max: usize) -> Result<()> {
    budget(max).value(value, 0)?;
    struct Count {
        size: usize,
        max: usize,
    }
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.size = self
                .size
                .checked_add(bytes.len())
                .ok_or(std::io::ErrorKind::OutOfMemory)?;
            if self.size > self.max {
                return Err(std::io::ErrorKind::OutOfMemory.into());
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Count { size: 0, max }, value).map_err(|_| Error)
}
fn program(nodes: &mut usize, depth: usize) -> Result<()> {
    *nodes += 1;
    check(*nodes <= MAX_PROGRAM && depth <= 12)
}
fn insert(out: &mut Map<String, Value>, path: &[String], value: Value) -> Result<()> {
    let (first, rest) = path.split_first().ok_or(Error)?;
    if rest.is_empty() {
        check(out.insert(first.clone(), value).is_none())
    } else {
        let child = out
            .entry(first.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        insert(child.as_object_mut().ok_or(Error)?, rest, value)
    }
}
impl Transform {
    pub(super) fn validate(&self) -> Result<()> {
        self.validate_inner(&mut 0, 0)
    }
    fn validate_inner(&self, n: &mut usize, d: usize) -> Result<()> {
        program(n, d)?;
        match self {
            Self::Identity => (),
            Self::Object { fields } => {
                check(!fields.is_empty() && fields.len() <= 128)?;
                let mut targets = Vec::new();
                for (k, f) in fields {
                    check(key(k) && !f.target.is_empty())?;
                    path(&f.target)?;
                    for target in &targets {
                        let target: &Vec<String> = target;
                        check(!target.starts_with(&f.target) && !f.target.starts_with(target))?;
                    }
                    targets.push(f.target.clone());
                    f.transform.validate_inner(n, d + 1)?;
                }
            }
            Self::Array { max_items, item } => {
                check((1..=4096).contains(max_items))?;
                item.validate_inner(n, d + 1)?;
            }
            Self::Enum { values } => {
                check(
                    !values.is_empty()
                        && values.len() <= 128
                        && values.iter().all(|(k, v)| key(k) && key(v))
                        && values.values().collect::<BTreeSet<_>>().len() == values.len(),
                )?;
            }
        };
        Ok(())
    }
    pub(super) fn apply(&self, input: &Value, max: usize) -> Result<Value> {
        bounded(input, max)?;
        let result = self.run(input, &mut budget(max), 0)?;
        bounded(&result, max)?;
        Ok(result)
    }
    fn run(&self, input: &Value, b: &mut Budget, d: usize) -> Result<Value> {
        b.step(d)?;
        match self {
            Self::Identity => b.clone_value(input, d),
            Self::Object { fields } => {
                let mut out = Map::new();
                let input = input.as_object().ok_or(Error)?;
                for (k, v) in input {
                    let field = fields.get(k).ok_or(Error)?;
                    b.bytes(field.target.iter().map(|s| s.len() + 3).sum())?;
                    insert(
                        &mut out,
                        &field.target,
                        field.transform.run(v, b, d + field.target.len())?,
                    )?;
                }
                Ok(Value::Object(out))
            }
            Self::Array { max_items, item } => {
                let input = input.as_array().ok_or(Error)?;
                check(input.len() <= *max_items)?;
                Ok(Value::Array(
                    input
                        .iter()
                        .map(|v| item.run(v, b, d + 1))
                        .collect::<Result<_>>()?,
                ))
            }
            Self::Enum { values } => {
                let value = values.get(input.as_str().ok_or(Error)?).ok_or(Error)?;
                b.bytes(value.len() + 2)?;
                Ok(Value::String(value.clone()))
            }
        }
    }
}
impl Projection {
    pub(super) fn requires_abi2(&self) -> bool {
        match self {
            Self::Tuple { .. } => true,
            Self::Object { fields } => {
                fields.is_empty() || fields.values().any(|field| field.value.requires_abi2())
            }
            Self::Array { item, .. } => item.requires_abi2(),
            _ => false,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.validate_inner(&mut 0, 0)
    }
    fn validate_inner(&self, n: &mut usize, d: usize) -> Result<()> {
        program(n, d)?;
        match self {
            Self::Tuple { items } => {
                check((1..=128).contains(&items.len()))?;
                for item in items {
                    item.validate_inner(n, d + 1)?;
                }
            }
            Self::Copy { path: p } => path(p)?,
            Self::Literal { value } => check(scalar(value))?,
            Self::Object { fields } => {
                check(fields.len() <= 128)?;
                for (k, f) in fields {
                    check(key(k))?;
                    // Optional means exactly a missing copy path, never swallowing type errors.
                    check(!f.optional || matches!(f.value, Self::Copy { .. }))?;
                    f.value.validate_inner(n, d + 1)?;
                }
            }
            Self::Array {
                path: p,
                max_items,
                item,
            } => {
                path(p)?;
                check((1..=4096).contains(max_items))?;
                item.validate_inner(n, d + 1)?;
            }
            Self::Enum { path: p, values } => {
                path(p)?;
                check(
                    !values.is_empty()
                        && values.len() <= 128
                        && values.iter().all(|(k, v)| key(k) && scalar(v)),
                )?;
            }
        };
        Ok(())
    }
    pub(super) fn apply(&self, input: &Value, max: usize) -> Result<Value> {
        bounded(input, max)?;
        let result = self.run(input, &mut budget(max), 0)?;
        bounded(&result, max)?;
        Ok(result)
    }
    fn run(&self, input: &Value, b: &mut Budget, d: usize) -> Result<Value> {
        b.step(d)?;
        match self {
            Self::Tuple { items } => Ok(Value::Array(
                items
                    .iter()
                    .map(|v| v.run(input, b, d + 1))
                    .collect::<Result<_>>()?,
            )),
            Self::Copy { path } => b.clone_value(read(input, path).ok_or(Error)?, d),
            Self::Literal { value } => b.clone_value(value, d),
            Self::Object { fields } => {
                let mut out = Map::new();
                for (k, f) in fields {
                    if f.optional {
                        if let Self::Copy { path } = &f.value {
                            if read(input, path).is_none() {
                                continue;
                            }
                        }
                    }
                    b.bytes(k.len() + 3)?;
                    out.insert(k.clone(), f.value.run(input, b, d + 1)?);
                }
                Ok(Value::Object(out))
            }
            Self::Array {
                path,
                max_items,
                item,
            } => {
                let values = read(input, path).and_then(Value::as_array).ok_or(Error)?;
                check(values.len() <= *max_items)?;
                Ok(Value::Array(
                    values
                        .iter()
                        .map(|v| item.run(v, b, d + 1))
                        .collect::<Result<_>>()?,
                ))
            }
            Self::Enum { path, values } => b.clone_value(
                values
                    .get(read(input, path).and_then(Value::as_str).ok_or(Error)?)
                    .ok_or(Error)?,
                d,
            ),
        }
    }
}
