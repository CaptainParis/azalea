use azalea_registry::{
    builtin::{
        Attribute, EnchantmentLevelBasedValueKind as LevelBasedValueKind,
        EnchantmentValueEffectKind as ValueEffectKind,
    },
    identifier::Identifier,
};
use simdnbt::{
    DeserializeError, FromNbtTag,
    borrow::{NbtCompound, NbtTag},
};

use crate::{
    attribute_modifier_operation::AttributeModifierOperation,
    registry_holder::{components::impl_from_effect_nbt_tag, get_in_compound},
};

#[derive(Clone, Debug)]
pub enum ValueEffect {
    Set { value: LevelBasedValue },
    Add { value: LevelBasedValue },
    Multiply { factor: LevelBasedValue },
    RemoveBinomial { chance: LevelBasedValue },
    AllOf { effects: Vec<ValueEffect> },
}

impl simdnbt::Deserialize for ValueEffect {
    fn from_compound(nbt: NbtCompound) -> Result<Self, DeserializeError> {
        let kind = get_in_compound(&nbt, "type")?;
        let value = match kind {
            ValueEffectKind::Set => {
                let value = get_in_compound(&nbt, "value")?;
                Self::Set { value }
            }
            ValueEffectKind::Add => {
                let value = get_in_compound(&nbt, "value")?;
                Self::Add { value }
            }
            ValueEffectKind::Multiply => {
                let factor = get_in_compound(&nbt, "factor")?;
                Self::Multiply { factor }
            }
            ValueEffectKind::RemoveBinomial => {
                let chance = get_in_compound(&nbt, "chance")?;
                Self::RemoveBinomial { chance }
            }
            ValueEffectKind::AllOf => {
                let effects = get_in_compound(&nbt, "effects")?;
                Self::AllOf { effects }
            }
        };
        Ok(value)
    }
}
impl_from_effect_nbt_tag!(ValueEffect);

#[derive(Clone, Debug, simdnbt::Deserialize)]
pub struct AttributeEffect {
    pub id: Identifier,
    pub attribute: Attribute,
    pub amount: LevelBasedValue,
    pub operation: AttributeModifierOperation,
}
impl_from_effect_nbt_tag!(AttributeEffect);

#[derive(Clone, Debug)]
pub enum LevelBasedValue {
    Constant(f32),
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
    LevelsSquared {
        added: f32,
    },
    Clamped {
        value: Box<LevelBasedValue>,
        min: f32,
        max: f32,
    },
    Fraction {
        numerator: Box<LevelBasedValue>,
        denominator: Box<LevelBasedValue>,
    },
    Lookup {
        values: Vec<f32>,
        fallback: Box<LevelBasedValue>,
    },
}
impl LevelBasedValue {
    pub fn calculate(&self, n: i32) -> f32 {
        match self {
            LevelBasedValue::Constant(v) => *v,
            LevelBasedValue::Linear {
                base,
                per_level_above_first,
                // int arithmetic wraps as in Java (levels come from the server,
                // and overflow would panic in debug builds)
            } => *base + *per_level_above_first * (n.wrapping_sub(1) as f32),
            LevelBasedValue::LevelsSquared { added } => n.wrapping_mul(n) as f32 + *added,
            // Mth.clamp, not f32::clamp: the bounds come from registry data,
            // and f32::clamp panics if min > max or either is NaN
            LevelBasedValue::Clamped { value, min, max } => {
                let v = value.calculate(n);
                if v < *min { *min } else { v.min(*max) }
            }
            LevelBasedValue::Fraction {
                numerator,
                denominator,
            } => {
                let value = denominator.calculate(n);
                if value == 0. {
                    0.
                } else {
                    // network-safe: f32 division, a zero denominator handled above
                    numerator.calculate(n) / value
                }
            }
            LevelBasedValue::Lookup { values, fallback } => values
                .get(n.wrapping_sub(1) as usize)
                .copied()
                .unwrap_or_else(|| fallback.calculate(n)),
        }
    }
}

impl Default for LevelBasedValue {
    fn default() -> Self {
        Self::Constant(0.)
    }
}

impl FromNbtTag for LevelBasedValue {
    fn from_nbt_tag(tag: NbtTag) -> Option<Self> {
        if let Some(f) = tag.float() {
            return Some(Self::Constant(f));
        }
        if let Some(c) = tag.compound() {
            return Self::from_compound(c).ok();
        }
        None
    }
}
impl LevelBasedValue {
    fn from_compound(nbt: NbtCompound) -> Result<Self, DeserializeError> {
        let kind = get_in_compound(&nbt, "type")?;
        let value = match kind {
            LevelBasedValueKind::Linear => {
                let base = get_in_compound(&nbt, "base")?;
                let per_level_above_first = get_in_compound(&nbt, "per_level_above_first")?;
                Self::Linear {
                    base,
                    per_level_above_first,
                }
            }
            LevelBasedValueKind::LevelsSquared => {
                let added = get_in_compound(&nbt, "added")?;
                Self::LevelsSquared { added }
            }
            LevelBasedValueKind::Clamped => {
                let value = Box::new(get_in_compound(&nbt, "value")?);
                let min = get_in_compound(&nbt, "min")?;
                let max = get_in_compound(&nbt, "max")?;
                Self::Clamped { value, min, max }
            }
            LevelBasedValueKind::Fraction => {
                let numerator = Box::new(get_in_compound(&nbt, "numerator")?);
                let denominator = Box::new(get_in_compound(&nbt, "denominator")?);
                Self::Fraction {
                    numerator,
                    denominator,
                }
            }
            LevelBasedValueKind::Lookup => {
                let values = nbt
                    .list("values")
                    .ok_or(DeserializeError::MissingField)?
                    .floats()
                    .ok_or(DeserializeError::MissingField)?;
                let fallback = Box::new(get_in_compound(&nbt, "fallback")?);
                Self::Lookup { values, fallback }
            }
        };
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::LevelBasedValue as V;

    #[test]
    fn server_values_and_levels_never_panic() {
        // min > max and NaN bounds (f32::clamp panics on both)
        let c = V::Clamped {
            value: Box::new(V::Constant(5.)),
            min: 10.,
            max: 1.,
        };
        assert_eq!(c.calculate(1), 10.);
        let c = V::Clamped {
            value: Box::new(V::Constant(5.)),
            min: f32::NAN,
            max: f32::NAN,
        };
        c.calculate(1);
        // levels that overflow i32 arithmetic
        let l = V::Linear {
            base: 1.,
            per_level_above_first: 1.,
        };
        l.calculate(i32::MIN);
        V::LevelsSquared { added: 0. }.calculate(i32::MAX);
        let lookup = V::Lookup {
            values: vec![1., 2.],
            fallback: Box::new(V::Constant(9.)),
        };
        assert_eq!(lookup.calculate(i32::MIN), 9.);
        assert_eq!(lookup.calculate(2), 2.);
    }
}
