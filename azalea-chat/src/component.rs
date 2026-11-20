#[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
use std::io::{self, Cursor, Write};
use std::{
    fmt::{self, Display},
    sync::LazyLock,
};

#[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
use azalea_buf::{AzBuf, BufReadError};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::{
    base_component::BaseComponent,
    style::{ChatFormatting, Style},
    text_component::TextComponent,
    translatable_component::{PrimitiveOrComponent, TranslatableComponent},
};

/// A chat component, basically anything you can see in chat.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum FormattedText {
    Text(TextComponent),
    Translatable(TranslatableComponent),
}

pub static DEFAULT_STYLE: LazyLock<Style> = LazyLock::new(|| Style {
    // network-safe: a constant (white is a color)
    color: Some(ChatFormatting::White.try_into().unwrap()),
    ..Style::default()
});

/// A chat component
impl FormattedText {
    pub fn get_base_mut(&mut self) -> &mut BaseComponent {
        match self {
            Self::Text(c) => &mut c.base,
            Self::Translatable(c) => &mut c.base,
        }
    }

    pub fn get_base(&self) -> &BaseComponent {
        match self {
            Self::Text(c) => &c.base,
            Self::Translatable(c) => &c.base,
        }
    }

    /// Add a component as a sibling of this one
    fn append(&mut self, sibling: FormattedText) {
        self.get_base_mut().siblings.push(sibling);
    }

    /// Get the "separator" component from the json
    fn parse_separator(
        json: &serde_json::Value,
    ) -> Result<Option<FormattedText>, serde_json::Error> {
        if let Some(separator) = json.get("separator") {
            return Ok(Some(FormattedText::deserialize(separator)?));
        }
        Ok(None)
    }

    #[cfg(feature = "simdnbt")]
    fn parse_separator_nbt(nbt: &simdnbt::borrow::NbtCompound) -> Option<FormattedText> {
        use simdnbt::FromNbtTag;

        if let Some(separator) = nbt.get("separator") {
            FormattedText::from_nbt_tag(separator)
        } else {
            None
        }
    }

    /// Render all components into a single `String`, using your custom
    /// closures to drive styling, text transformation, and final cleanup.
    ///
    /// # Type parameters
    ///
    /// - `F`: `(running, component, default) -> (prefix, suffix)` for
    ///   per-component styling
    /// - `S`: `&str -> String` for text tweaks (escaping, mapping, etc.)
    /// - `C`: `&final_running_style -> String` for any trailing cleanup
    ///
    /// # Arguments
    ///
    /// - `style_formatter`: how to open/close each component's style
    /// - `text_formatter`: how to turn raw text into output text
    /// - `cleanup_formatter`: emit after all components (e.g. reset codes)
    /// - `default_style`: where to reset when a component's `reset` is true
    ///
    /// # Example
    ///
    /// ```rust
    /// use azalea_chat::{FormattedText, DEFAULT_STYLE};
    /// use serde::de::Deserialize;
    ///
    /// let component = FormattedText::deserialize(&serde_json::json!({
    ///    "text": "Hello, world!",
    ///    "color": "red",
    /// })).unwrap();
    ///
    /// let ansi = component.to_custom_format(
    ///     |running, new| (running.compare_ansi(new), "".to_owned()),
    ///     |text| text.to_string(),
    ///     |style| {
    ///         if !style.is_empty() {
    ///             "\u{1b}[m".to_owned()
    ///         } else {
    ///             "".to_owned()
    ///         }
    ///     },
    ///     &DEFAULT_STYLE,
    /// );
    /// println!("{ansi}");
    /// ```
    pub fn to_custom_format<F, S, C>(
        &self,
        mut style_formatter: F,
        mut text_formatter: S,
        mut cleanup_formatter: C,
        default_style: &Style,
    ) -> String
    where
        F: FnMut(&Style, &Style) -> (String, String),
        S: FnMut(&str) -> String,
        C: FnMut(&Style) -> String,
    {
        let mut output = String::new();

        let mut running_style = Style::default();
        self.to_custom_format_recursive(
            &mut output,
            &mut style_formatter,
            &mut text_formatter,
            &default_style.clone(),
            &mut running_style,
        );
        output.push_str(&cleanup_formatter(&running_style));

        output
    }

    fn to_custom_format_recursive<F, S>(
        &self,
        output: &mut String,
        style_formatter: &mut F,
        text_formatter: &mut S,
        parent_style: &Style,
        running_style: &mut Style,
    ) where
        F: FnMut(&Style, &Style) -> (String, String),
        S: FnMut(&str) -> String,
    {
        let component_style = &self.get_base().style;
        let new_style = parent_style.merged_for_render(component_style);

        let mut append_text = |text: &str| {
            if !text.is_empty() {
                let (formatted_style_prefix, formatted_style_suffix) =
                    style_formatter(running_style, &new_style);
                let formatted_text = text_formatter(text);

                output.push_str(&formatted_style_prefix);
                output.push_str(&formatted_text);
                output.push_str(&formatted_style_suffix);

                *running_style = new_style.clone();
            }
        };

        match &self {
            Self::Text(c) => {
                append_text(&c.text);
            }
            Self::Translatable(c) => match c.read() {
                Ok(c) => {
                    FormattedText::Text(c).to_custom_format_recursive(
                        output,
                        style_formatter,
                        text_formatter,
                        &new_style,
                        running_style,
                    );
                }
                Err(_) => {
                    append_text(&c.key);
                }
            },
        };

        for sibling in &self.get_base().siblings {
            sibling.to_custom_format_recursive(
                output,
                style_formatter,
                text_formatter,
                &new_style,
                running_style,
            );
        }
    }

    /// Convert this component into an
    /// [ANSI string](https://en.wikipedia.org/wiki/ANSI_escape_code).
    ///
    /// This is the same as [`FormattedText::to_ansi`], but you can specify a
    /// default [`Style`] to use.
    pub fn to_ansi_with_custom_style(&self, default_style: &Style) -> String {
        self.to_custom_format(
            |running, new| (running.compare_ansi(new), "".to_owned()),
            |text| text.to_owned(),
            |style| if !style.is_empty() { "\u{1b}[m" } else { "" }.to_owned(),
            default_style,
        )
    }

    /// Convert this component into an
    /// [ANSI string](https://en.wikipedia.org/wiki/ANSI_escape_code), so you
    /// can print it to your terminal and get styling.
    ///
    /// This is technically a shortcut for
    /// [`FormattedText::to_ansi_with_custom_style`] with a default [`Style`]
    /// colored white.
    ///
    /// If you don't want the result to be styled at all, use
    /// [`Self::to_string`](#method.fmt-1).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use azalea_chat::FormattedText;
    /// use serde::de::Deserialize;
    ///
    /// let component = FormattedText::deserialize(&serde_json::json!({
    ///    "text": "Hello, world!",
    ///    "color": "red",
    /// })).unwrap();
    ///
    /// println!("{}", component.to_ansi());
    /// ```
    pub fn to_ansi(&self) -> String {
        self.to_ansi_with_custom_style(&DEFAULT_STYLE)
    }

    /// Similar to [`Self::to_ansi`] but renders the result as HTML instead.
    pub fn to_html(&self) -> String {
        self.to_custom_format(
            |running, new| {
                (
                    format!(
                        "<span style=\"{}\">",
                        running.merged_with(new).get_html_style()
                    ),
                    "</span>".to_owned(),
                )
            },
            |text| {
                text.replace("&", "&amp;")
                    .replace("<", "&lt;")
                    // usually unnecessary but good for compatibility
                    .replace(">", "&gt;")
                    .replace("\n", "<br>")
            },
            |_| "".to_owned(),
            &DEFAULT_STYLE,
        )
    }
}

impl IntoIterator for FormattedText {
    type Item = FormattedText;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    /// Recursively call the function for every component in this component
    fn into_iter(self) -> Self::IntoIter {
        let base = self.get_base();
        let siblings = base.siblings.clone();
        let mut v: Vec<FormattedText> = Vec::with_capacity(siblings.len() + 1);
        v.push(self);
        for sibling in siblings {
            v.extend(sibling);
        }

        v.into_iter()
    }
}

impl<'de> Deserialize<'de> for FormattedText {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let json: serde_json::Value = serde::Deserialize::deserialize(de)?;

        // we create a component that we might add siblings to
        let mut component: FormattedText;

        // if it's primitive, make it a text component
        if !json.is_array() && !json.is_object() {
            return Ok(FormattedText::Text(TextComponent::new(
                json.as_str().unwrap_or("").to_owned(),
            )));
        }
        // if it's an object, do things with { text } and stuff
        else if json.is_object() {
            if let Some(text) = json.get("text") {
                let text = text.as_str().unwrap_or("").to_owned();
                component = FormattedText::Text(TextComponent::new(text));
            } else if let Some(translate) = json.get("translate") {
                let translate = translate
                    .as_str()
                    .ok_or_else(|| de::Error::custom("\"translate\" must be a string"))?
                    .into();
                let fallback = if let Some(fallback) = json.get("fallback") {
                    Some(
                        fallback
                            .as_str()
                            .ok_or_else(|| de::Error::custom("\"fallback\" must be a string"))?
                            .to_owned(),
                    )
                } else {
                    None
                };
                if let Some(with) = json.get("with") {
                    let with_array = with
                        .as_array()
                        .ok_or_else(|| de::Error::custom("\"with\" must be an array"))?
                        .iter()
                        .map(|item| {
                            PrimitiveOrComponent::deserialize(item).map_err(de::Error::custom)
                        })
                        .collect::<Result<Vec<PrimitiveOrComponent>, _>>()?;

                    component = FormattedText::Translatable(TranslatableComponent::with_fallback(
                        translate, fallback, with_array,
                    ));
                } else {
                    // if it doesn't have a "with", just have the with_array be
                    // empty
                    component = FormattedText::Translatable(TranslatableComponent::with_fallback(
                        translate,
                        fallback,
                        Vec::new(),
                    ));
                }
            } else if let Some(score) = json.get("score") {
                // object = GsonHelper.getAsJsonObject(jsonObject, "score");
                if score.get("name").is_none() || score.get("objective").is_none() {
                    return Err(de::Error::missing_field(
                        "A score component needs at least a name and an objective",
                    ));
                }
                // TODO
                return Err(de::Error::custom(
                    "score text components aren't yet supported",
                ));
            } else if json.get("selector").is_some() {
                return Err(de::Error::custom(
                    "selector text components aren't yet supported",
                ));
            } else if json.get("keybind").is_some() {
                return Err(de::Error::custom(
                    "keybind text components aren't yet supported",
                ));
            } else if json.get("object").is_some() {
                return Err(de::Error::custom(
                    "object text components aren't yet supported",
                ));
            } else {
                let Some(_nbt) = json.get("nbt") else {
                    return Err(de::Error::custom(
                        format!("Don't know how to turn {json} into a FormattedText").as_str(),
                    ));
                };
                let _separator =
                    FormattedText::parse_separator(&json).map_err(de::Error::custom)?;

                let _interpret = match json.get("interpret") {
                    // not a bool: false (this unwrapped an Err, a panic on
                    // a JSON component from the server)
                    Some(v) => v.as_bool().unwrap_or(false),
                    None => false,
                };
                if let Some(_block) = json.get("block") {}
                return Err(de::Error::custom(
                    "nbt text components aren't yet supported",
                ));
            }
            if let Some(extra) = json.get("extra") {
                let Some(extra) = extra.as_array() else {
                    return Err(de::Error::custom("Extra isn't an array"));
                };
                if extra.is_empty() {
                    return Err(de::Error::custom("Unexpected empty array of components"));
                }
                for extra_component in extra {
                    let sibling =
                        FormattedText::deserialize(extra_component).map_err(de::Error::custom)?;
                    component.append(sibling);
                }
            }

            let style = Style::deserialize(&json);
            *component.get_base_mut().style = style;

            return Ok(component);
        }
        // ok so it's not an object, if it's an array deserialize every item
        else if !json.is_array() {
            return Err(de::Error::custom(
                format!("Don't know how to turn {json} into a FormattedText").as_str(),
            ));
        }
        let Some(json_array) = json.as_array() else {
            return Err(de::Error::custom("Expected an array"));
        };
        // the first item in the array is the one that we're gonna return, the
        // others are siblings
        let Some((first, rest)) = json_array.split_first() else {
            return Ok(FormattedText::default());
        };
        let mut component = FormattedText::deserialize(first).map_err(de::Error::custom)?;
        for item in rest {
            component.append(FormattedText::deserialize(item).map_err(de::Error::custom)?);
        }
        Ok(component)
    }
}

#[cfg(feature = "simdnbt")]
impl simdnbt::Serialize for FormattedText {
    fn to_compound(self) -> simdnbt::owned::NbtCompound {
        component_compound(&self)
    }
}

/// The full (uncollapsed) compound form of a component, as vanilla 1.21.4
/// `ComponentSerialization.CODEC` writes it: contents fields, then `extra`
/// (omitted when empty), then the style fields.
#[cfg(feature = "simdnbt")]
pub(crate) fn component_compound(component: &FormattedText) -> simdnbt::owned::NbtCompound {
    use simdnbt::Serialize;

    let mut compound = simdnbt::owned::NbtCompound::new();
    match component {
        FormattedText::Text(c) => {
            compound.insert("text", c.text.clone());
        }
        FormattedText::Translatable(c) => {
            compound.insert("translate", c.key.clone());
            if let Some(fallback) = &c.fallback {
                compound.insert("fallback", fallback.clone());
            }
            // TranslatableContents.adjustArgs: no args means no `with` field
            if !c.args.is_empty() {
                use simdnbt::ToNbtTag;
                compound.insert(
                    "with",
                    collect_nbt_list(c.args.iter().cloned().map(|a| a.to_nbt_tag()).collect()),
                );
            }
        }
    }
    let base = component.get_base();
    if !base.siblings.is_empty() {
        compound.insert(
            "extra",
            collect_nbt_list(
                base.siblings
                    .iter()
                    .map(FormattedText::to_vanilla_nbt_tag)
                    .collect(),
            ),
        );
    }
    compound.extend((*base.style).clone().to_compound());
    compound
}

/// Builds a list tag the way vanilla 1.21.4 `NbtOps.createList` does: bytes,
/// ints and longs become arrays, other homogeneous lists stay typed lists, and
/// a list that starts with a compound or mixes types wraps every element that
/// is not a plain compound in `{"": element}`.
#[cfg(feature = "simdnbt")]
pub(crate) fn collect_nbt_list(tags: Vec<simdnbt::owned::NbtTag>) -> simdnbt::owned::NbtTag {
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

    fn wrap_if_needed(tag: NbtTag) -> NbtCompound {
        match tag {
            NbtTag::Compound(c) if !(c.len() == 1 && c.get("").is_some()) => c,
            other => {
                let mut c = NbtCompound::new();
                c.insert("", other);
                c
            }
        }
    }
    let Some(first) = tags.first() else {
        return NbtTag::List(NbtList::Empty);
    };
    let first_id = first.id();
    let homogeneous = tags.iter().all(|t| t.id() == first_id);
    if matches!(first, NbtTag::Compound(_)) || !homogeneous {
        return NbtTag::List(NbtList::Compound(
            tags.into_iter().map(wrap_if_needed).collect(),
        ));
    }
    // every tag has first's type (`homogeneous` above), so the expects hold
    match first {
        NbtTag::Byte(_) => NbtTag::ByteArray(
            tags.into_iter()
                .map(|t| t.into_byte().expect("homogeneous") as u8) // network-safe: homogeneous
                .collect(),
        ),
        NbtTag::Int(_) => NbtTag::IntArray(
            tags.into_iter()
                .map(|t| t.into_int().expect("homogeneous")) // network-safe: homogeneous
                .collect(),
        ),
        NbtTag::Long(_) => NbtTag::LongArray(
            tags.into_iter()
                .map(|t| t.into_long().expect("homogeneous")) // network-safe: homogeneous
                .collect(),
        ),
        _ => NbtTag::List(NbtList::from(tags)),
    }
}

#[cfg(feature = "simdnbt")]
impl FormattedText {
    /// The tag vanilla 1.21.4 writes for this component: a plain string when it
    /// is literal text with no style and no siblings
    /// (`Component.tryCollapseToString`), else the full compound.
    pub fn to_vanilla_nbt_tag(&self) -> simdnbt::owned::NbtTag {
        if let FormattedText::Text(c) = self
            && c.base.siblings.is_empty()
            && c.base.style.is_unstyled()
        {
            return simdnbt::owned::NbtTag::String(c.text.as_str().into());
        }
        simdnbt::owned::NbtTag::Compound(component_compound(self))
    }
}

#[cfg(feature = "simdnbt")]
impl simdnbt::FromNbtTag for FormattedText {
    fn from_nbt_tag(tag: simdnbt::borrow::NbtTag) -> Option<Self> {
        // if it's a string, return a text component with that string
        if let Some(string) = tag.string() {
            Some(FormattedText::from_nbt_string(string))
        }
        // if it's a compound, make it do things with { text } and stuff
        // simdnbt::borrow::NbtTag::Compound(compound) => {
        else if let Some(compound) = tag.compound() {
            FormattedText::from_nbt_compound(compound)
        }
        // ok so it's not a compound, if it's a list deserialize every item
        else if let Some(list) = tag.list() {
            FormattedText::from_nbt_list(list)
        } else {
            Some(FormattedText::Text(TextComponent::new("".to_owned())))
        }
    }
}

#[cfg(feature = "simdnbt")]
impl FormattedText {
    fn from_nbt_string(s: &simdnbt::Mutf8Str) -> Self {
        FormattedText::from(s)
    }
    fn from_nbt_list(list: simdnbt::borrow::NbtList) -> Option<FormattedText> {
        use tracing::debug;

        let mut component;
        if let Some(compounds) = list.compounds() {
            component = FormattedText::from_nbt_compound(compounds.first()?)?;
            for compound in compounds.into_iter().skip(1) {
                component.append(FormattedText::from_nbt_compound(compound)?);
            }
        } else if let Some(strings) = list.strings() {
            component = FormattedText::from(*(strings.first()?));
            for &string in strings.iter().skip(1) {
                component.append(FormattedText::from(string));
            }
        } else {
            debug!("couldn't parse {list:?} as FormattedText");
            return None;
        }
        Some(component)
    }

    pub fn from_nbt_compound(compound: simdnbt::borrow::NbtCompound) -> Option<Self> {
        use simdnbt::{Deserialize, FromNbtTag};
        use tracing::{trace, warn};

        let mut component: FormattedText;

        if let Some(text) = compound.get("text") {
            let text = text.string().unwrap_or_default().to_string();
            component = FormattedText::Text(TextComponent::new(text));
        } else if let Some(translate) = compound.get("translate") {
            let translate = translate.string()?.into();
            // TranslatableContents.CODEC: lenientOptionalFieldOf("fallback")
            let fallback = compound.string("fallback").map(|s| s.to_string());
            if let Some(with) = compound.get("with") {
                let mut with_array = Vec::new();
                // NbtOps writes lists of ints, bytes and longs as array tags
                if let Some(ints) = with.int_array() {
                    with_array.extend(ints.into_iter().map(PrimitiveOrComponent::Integer));
                } else if let Some(bytes) = with.byte_array() {
                    with_array.extend(bytes.iter().map(|&b| PrimitiveOrComponent::Boolean(b != 0)));
                } else if let Some(longs) = with.long_array() {
                    with_array.extend(longs.into_iter().map(PrimitiveOrComponent::Long));
                } else {
                    let with_list = with.list()?;
                    if with_list.empty() {
                    } else if let Some(with) = with_list.strings() {
                        for item in with {
                            with_array.push(PrimitiveOrComponent::String(item.to_string()));
                        }
                    } else if let Some(with) = with_list.ints() {
                        for item in with {
                            with_array.push(PrimitiveOrComponent::Integer(item));
                        }
                    } else if let Some(with) = with_list.longs() {
                        for item in with {
                            with_array.push(PrimitiveOrComponent::Long(item));
                        }
                    // ARG_CODEC reads every primitive; NbtOps writes a
                    // homogeneous list of shorts, floats or doubles as a
                    // typed list (and of bytes as an array, above)
                    } else if let Some(with) = with_list.shorts() {
                        with_array.extend(with.into_iter().map(PrimitiveOrComponent::Short));
                    } else if let Some(with) = with_list.floats() {
                        with_array.extend(with.into_iter().map(PrimitiveOrComponent::Float));
                    } else if let Some(with) = with_list.doubles() {
                        with_array.extend(with.into_iter().map(PrimitiveOrComponent::Double));
                    } else if let Some(with) = with_list.bytes() {
                        with_array.extend(
                            with.iter().map(|&b| PrimitiveOrComponent::Boolean(b != 0)),
                        );
                    } else if let Some(with) = with_list.compounds() {
                        for item in with {
                            // if it's a string component with no styling and no
                            // siblings, just add a string to with_array.
                            // otherwise, add the component to the array
                            if let Some(primitive) = item.get("") {
                                // minecraft does this sometimes, for example
                                // for the /give system messages
                                if let Some(b) = primitive.byte() {
                                    // interpreted as boolean
                                    with_array.push(PrimitiveOrComponent::Boolean(b != 0));
                                } else if let Some(s) = primitive.short() {
                                    with_array.push(PrimitiveOrComponent::Short(s));
                                } else if let Some(i) = primitive.int() {
                                    with_array.push(PrimitiveOrComponent::Integer(i));
                                } else if let Some(l) = primitive.long() {
                                    with_array.push(PrimitiveOrComponent::Long(l));
                                } else if let Some(f) = primitive.float() {
                                    with_array.push(PrimitiveOrComponent::Float(f));
                                } else if let Some(d) = primitive.double() {
                                    with_array.push(PrimitiveOrComponent::Double(d));
                                } else if let Some(s) = primitive.string() {
                                    with_array.push(PrimitiveOrComponent::String(s.to_string()));
                                } else {
                                    warn!(
                                        "couldn't parse {item:?} as FormattedText because it has a disallowed primitive"
                                    );
                                    with_array.push(PrimitiveOrComponent::String("?".to_owned()));
                                }
                            } else if let Some(c) = FormattedText::from_nbt_compound(item) {
                                // parse each item once: parsing it again here
                                // made a translate/with chain of depth d cost
                                // 2^d parses
                                match c {
                                    FormattedText::Text(text_component)
                                        if text_component.base.siblings.is_empty()
                                            && text_component.base.style.is_unstyled() =>
                                    {
                                        with_array.push(PrimitiveOrComponent::String(
                                            text_component.text,
                                        ));
                                    }
                                    c => with_array.push(PrimitiveOrComponent::FormattedText(c)),
                                }
                            } else {
                                warn!("couldn't parse {item:?} as FormattedText");
                                with_array.push(PrimitiveOrComponent::String("?".to_owned()));
                            }
                        }
                    } else {
                        warn!(
                            "couldn't parse {with:?} as FormattedText because it's not a list of compounds"
                        );
                        return None;
                    }
                }
                component = FormattedText::Translatable(TranslatableComponent::with_fallback(
                    translate, fallback, with_array,
                ));
            } else {
                // if it doesn't have a "with", just have the with_array be
                // empty
                component = FormattedText::Translatable(TranslatableComponent::with_fallback(
                    translate,
                    fallback,
                    Vec::new(),
                ));
            }
        } else if let Some(score) = compound.compound("score") {
            if score.get("name").is_none() || score.get("objective").is_none() {
                trace!("A score component needs at least a name and an objective");
                return None;
            }
            // TODO: implement these
            return None;
        } else if compound.get("selector").is_some() {
            trace!("selector text components aren't supported");
            return None;
        } else if compound.get("keybind").is_some() {
            trace!("keybind text components aren't supported");
            return None;
        } else if compound.get("object").is_some() {
            trace!("object text components aren't supported");
            return None;
        } else if let Some(tag) = compound.get("") {
            return FormattedText::from_nbt_tag(tag);
        } else {
            let _nbt = compound.get("nbt")?;
            let _separator = FormattedText::parse_separator_nbt(&compound)?;

            let _interpret = match compound.get("interpret") {
                Some(v) => v.byte().unwrap_or_default() != 0,
                None => false,
            };
            if let Some(_block) = compound.get("block") {}
            trace!("nbt text components aren't yet supported");
            return None;
        }
        if let Some(extra) = compound.get("extra") {
            // if it's an array, deserialize every item
            if let Some(items) = extra.list() {
                if let Some(items) = items.compounds() {
                    for item in items {
                        component.append(FormattedText::from_nbt_compound(item)?);
                    }
                } else if let Some(items) = items.strings() {
                    for item in items {
                        component.append(FormattedText::from_nbt_string(item));
                    }
                } else if let Some(items) = items.lists() {
                    for item in items {
                        component.append(FormattedText::from_nbt_list(item)?);
                    }
                } else {
                    warn!(
                        "couldn't parse {items:?} as FormattedText because it's not a list of compounds or strings"
                    );
                }
            } else {
                component.append(FormattedText::from_nbt_tag(extra)?);
            }
        }

        let base_style = Style::from_compound(compound).ok()?;
        let new_style = &mut component.get_base_mut().style;
        // by value: cloning the hover event at every level of a nested chain
        // was quadratic
        **new_style = std::mem::take(&mut **new_style).merged_with_owned(base_style);

        Some(component)
    }
}

#[cfg(feature = "simdnbt")]
impl From<&simdnbt::Mutf8Str> for FormattedText {
    fn from(s: &simdnbt::Mutf8Str) -> Self {
        FormattedText::Text(TextComponent::new(s.to_string()))
    }
}

#[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
impl AzBuf for FormattedText {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        use simdnbt::FromNbtTag;
        use tracing::trace;

        azalea_buf::nbt::check_network_nbt(buf)?;
        let nbt = simdnbt::borrow::read_optional_tag(buf)?;
        trace!(
            "Reading NBT for FormattedText: {:?}",
            nbt.as_ref().map(|n| n.as_tag().to_owned())
        );
        match nbt {
            Some(nbt) => FormattedText::from_nbt_tag(nbt.as_tag()).ok_or(BufReadError::Custom(
                "couldn't convert nbt to chat message".to_owned(),
            )),
            _ => Ok(FormattedText::default()),
        }
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        // network NBT: tag type id, then the unnamed payload
        let mut out = Vec::new();
        self.to_vanilla_nbt_tag().write(&mut out);
        buf.write_all(&out)
    }
}

impl From<String> for FormattedText {
    fn from(s: String) -> Self {
        FormattedText::Text(TextComponent {
            text: s,
            base: BaseComponent::default(),
        })
    }
}
impl From<&str> for FormattedText {
    fn from(s: &str) -> Self {
        Self::from(s.to_owned())
    }
}

impl Display for FormattedText {
    /// Render the text in the component but without any formatting/styling.
    ///
    /// If you want the text to be styled, consider using [`Self::to_ansi`] or
    /// [`Self::to_html`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormattedText::Text(c) => c.fmt(f),
            FormattedText::Translatable(c) => c.fmt(f),
        }
    }
}

impl Default for FormattedText {
    fn default() -> Self {
        FormattedText::Text(TextComponent::default())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::style::TextColor;

    // fuzz finding (chat_component, WP2 gate): `with` lists of shorts, floats
    // or doubles (NbtOps writes a homogeneous list of them as a typed list)
    // didn't read back, so a component with such an argument (or nested in
    // one) lost it on our own round trip. Vanilla's ARG_CODEC reads every
    // primitive in the list.
    #[cfg(feature = "azalea-buf")]
    #[test]
    fn primitive_translate_args_round_trip() {
        use azalea_buf::AzBuf;
        for args in [
            vec![PrimitiveOrComponent::Float(0.)],
            vec![PrimitiveOrComponent::Short(256), PrimitiveOrComponent::Short(-1)],
            vec![PrimitiveOrComponent::Double(1.5)],
            vec![PrimitiveOrComponent::Boolean(true)],
            vec![PrimitiveOrComponent::Integer(3)],
            vec![PrimitiveOrComponent::Long(7)],
            vec![
                PrimitiveOrComponent::String("a".to_owned()),
                PrimitiveOrComponent::FormattedText(FormattedText::Translatable(
                    TranslatableComponent::new(
                        "chat.square_brackets".to_owned(),
                        vec![PrimitiveOrComponent::Float(8.)],
                    ),
                )),
            ],
        ] {
            let text = FormattedText::Translatable(TranslatableComponent::new(
                "multiplayer.player.joined".to_owned(),
                args,
            ));
            let mut bytes = Vec::new();
            text.azalea_write(&mut bytes).unwrap();
            let back = FormattedText::azalea_read(&mut std::io::Cursor::new(&bytes[..]))
                .unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!(back, text);
        }
    }

    #[test]
    fn deserialize_translation() {
        let j: Value =
            serde_json::from_str(r#"{"translate": "translation.test.args", "with": ["a", "b"]}"#)
                .unwrap();
        let component = FormattedText::deserialize(&j).unwrap();
        assert_eq!(
            component,
            FormattedText::Translatable(TranslatableComponent::new(
                "translation.test.args".to_owned(),
                vec![
                    PrimitiveOrComponent::String("a".to_owned()),
                    PrimitiveOrComponent::String("b".to_owned())
                ]
            ))
        );
    }

    #[test]
    fn deserialize_translation_invalid_arguments() {
        let j: Value =
            serde_json::from_str(r#"{"translate": "translation.test.args", "with": {}}"#).unwrap();
        assert!(FormattedText::deserialize(&j).is_err());
    }

    #[test]
    fn deserialize_translation_fallback() {
        let j: Value = serde_json::from_str(r#"{"translate": "translation.test.undefined", "fallback": "fallback: %s", "with": ["a"]}"#).unwrap();
        let component = FormattedText::deserialize(&j).unwrap();
        assert_eq!(
            component,
            FormattedText::Translatable(TranslatableComponent::with_fallback(
                "translation.test.undefined".to_owned(),
                Some("fallback: %s".to_owned()),
                vec![PrimitiveOrComponent::String("a".to_owned())]
            ))
        );
    }

    #[test]
    fn deserialize_translation_invalid_fallback() {
        let j: Value = serde_json::from_str(
            r#"{"translate": "translation.test.undefined", "fallback": {"text": "invalid"}}"#,
        )
        .unwrap();
        assert!(FormattedText::deserialize(&j).is_err());
    }
    #[test]
    fn deserialize_translation_primitive_args() {
        let j: Value = serde_json::from_str(
            r#"{"translate":"commands.list.players", "with": [1, 65536, "<players>", {"text": "unused", "color": "red"}]}"#,
        )
        .unwrap();
        assert_eq!(
            FormattedText::deserialize(&j).unwrap(),
            FormattedText::Translatable(TranslatableComponent::new(
                "commands.list.players".to_owned(),
                vec![
                    PrimitiveOrComponent::Short(1),
                    PrimitiveOrComponent::Integer(65536),
                    PrimitiveOrComponent::String("<players>".to_owned()),
                    PrimitiveOrComponent::FormattedText(FormattedText::Text(
                        TextComponent::new("unused")
                            .with_style(Style::new().color(Some(TextColor::parse("red").unwrap())))
                    ))
                ]
            ))
        );
    }

    #[test]
    fn json_nbt_component_with_a_non_bool_interpret_is_an_error() {
        // this unwrapped an Err (a panic)
        let j = serde_json::json!({"nbt": "a", "interpret": 5});
        assert!(FormattedText::deserialize(&j).is_err());
        let j = serde_json::json!(["a", {"text": "b"}]);
        assert_eq!(FormattedText::deserialize(&j).unwrap().to_string(), "ab");
        let j = serde_json::json!([]);
        assert_eq!(FormattedText::deserialize(&j).unwrap(), FormattedText::default());
    }

    #[test]
    fn test_translatable_with_color_inheritance() {
        let json = serde_json::json!({
            "translate": "advancements.story.smelt_iron.title",
            "color": "green",
            "with": [{"text": "Acquire Hardware"}]
        });
        let component = FormattedText::deserialize(&json).unwrap();
        let ansi = component.to_ansi();
        assert!(ansi.contains("\u{1b}[38;2;85;255;85m"));
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    fn round_trip(c: &FormattedText) -> (Vec<u8>, FormattedText) {
        let mut buf = Vec::new();
        c.azalea_write(&mut buf).unwrap();
        let back = FormattedText::azalea_read(&mut Cursor::new(&buf[..])).unwrap();
        (buf, back)
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn nbt_plain_text_is_a_string_tag() {
        // Component.tryCollapseToString: literal text, no style, no siblings
        let (buf, back) = round_trip(&FormattedText::from("azalea-hunt rig"));
        assert_eq!(buf[0], 8, "string tag");
        assert_eq!(back, FormattedText::from("azalea-hunt rig"));
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn nbt_translate_arg_keeps_insertion_click_and_hover() {
        use crate::{click_event::ClickEvent, hover_event::HoverEvent};

        // what Paper 1.21.4 sends for "CorpusBot1 joined the game"
        let mut name = TextComponent::new("CorpusBot1".to_owned());
        name.base.style = Box::new(Style {
            insertion: Some("CorpusBot1".to_owned()),
            click_event: Some(ClickEvent::SuggestCommand {
                command: "/tell CorpusBot1 ".to_owned(),
            }),
            hover_event: Some(HoverEvent::ShowEntity {
                kind: "minecraft:player".to_owned(),
                id: "76c4497e-481e-3492-89da-2c512999bba5".to_owned(),
                name: Some(Box::new(FormattedText::from("CorpusBot1"))),
            }),
            ..Style::default()
        });
        let mut joined = TranslatableComponent::new(
            "multiplayer.player.joined".to_owned(),
            vec![PrimitiveOrComponent::FormattedText(FormattedText::Text(
                name,
            ))],
        );
        joined.base.style = Box::new(Style {
            color: TextColor::parse("yellow"),
            ..Style::default()
        });
        let joined = FormattedText::Translatable(joined);
        let (buf, back) = round_trip(&joined);
        assert_eq!(back, joined);
        // the uuid is written as vanilla's int array
        let ints: [u8; 4] = 0x76c4497e_u32.to_be_bytes();
        assert!(buf.windows(4).any(|w| w == ints));
    }

    /// A `translate`/`with` chain as deep as network NBT allows: each level
    /// is a compound holding a `with` list, so 255 levels plus the leaf use
    /// 511 of the 512 levels.
    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    pub(crate) fn deep_translate_chain(levels: usize) -> Vec<u8> {
        // network NBT: compound tag id, then the unnamed payload
        let mut buf = vec![10u8];
        let key = |buf: &mut Vec<u8>, ty: u8, name: &str| {
            buf.push(ty);
            buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
            buf.extend_from_slice(name.as_bytes());
        };
        let string = |buf: &mut Vec<u8>, s: &str| {
            buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
            buf.extend_from_slice(s.as_bytes());
        };
        for _ in 0..levels {
            key(&mut buf, 8, "translate");
            string(&mut buf, "chat.type.text");
            // styled, so the item isn't collapsed to a plain string
            key(&mut buf, 1, "bold");
            buf.push(1);
            key(&mut buf, 9, "with");
            buf.push(10);
            buf.extend_from_slice(&1i32.to_be_bytes());
        }
        key(&mut buf, 8, "text");
        string(&mut buf, "leaf");
        // close the leaf compound, then each level's compound (the list
        // holding it has exactly one element, so it closes by count)
        buf.extend(std::iter::repeat_n(0u8, levels + 1));
        buf
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn deep_translate_with_chain_is_linear() {
        // bot threads have a 16 MB stack
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(deep_translate_with_chain_is_linear_inner)
            .unwrap()
            .join()
            .unwrap();
    }
    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    fn deep_translate_with_chain_is_linear_inner() {
        let levels = 255;
        let buf = deep_translate_chain(levels);
        let start = std::time::Instant::now();
        let c = FormattedText::azalea_read(&mut Cursor::new(&buf[..])).unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(10),
            "took {elapsed:?}"
        );

        let mut depth = 0;
        let mut cur = &c;
        while let FormattedText::Translatable(t) = cur {
            depth += 1;
            match &t.args[..] {
                [PrimitiveOrComponent::FormattedText(next)] => cur = next,
                [PrimitiveOrComponent::String(s)] => {
                    assert_eq!(s, "leaf");
                    break;
                }
                other => panic!("unexpected args {other:?}"),
            }
        }
        assert_eq!(depth, levels);
    }

    /// `levels` components, each with a show_text hover event holding the
    /// next; the innermost has `width` text siblings. Every level holds the
    /// whole rest of the tree in its hover event.
    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    fn deep_hover_chain(levels: usize, width: usize) -> Vec<u8> {
        let mut buf = vec![10u8];
        let key = |buf: &mut Vec<u8>, ty: u8, name: &str| {
            buf.push(ty);
            buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
            buf.extend_from_slice(name.as_bytes());
        };
        let string = |buf: &mut Vec<u8>, s: &str| {
            buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
            buf.extend_from_slice(s.as_bytes());
        };
        for _ in 0..levels {
            key(&mut buf, 8, "text");
            string(&mut buf, "x");
            key(&mut buf, 10, "hoverEvent");
            key(&mut buf, 8, "action");
            string(&mut buf, "show_text");
            key(&mut buf, 10, "contents");
        }
        key(&mut buf, 8, "text");
        string(&mut buf, "leaf");
        key(&mut buf, 9, "extra");
        buf.push(10);
        buf.extend_from_slice(&(width as i32).to_be_bytes());
        for _ in 0..width {
            key(&mut buf, 8, "text");
            string(&mut buf, "a");
            buf.push(0);
        }
        // the leaf, then each level's contents and hoverEvent compounds and
        // the level itself
        buf.push(0);
        buf.extend(std::iter::repeat_n(0u8, levels * 2));
        buf
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    fn on_bot_thread(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap();
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn deep_hover_chain_is_linear() {
        on_bot_thread(|| {
            let parse = |levels| {
                let buf = deep_hover_chain(levels, 12_000);
                let start = std::time::Instant::now();
                FormattedText::azalea_read(&mut Cursor::new(&buf[..])).unwrap();
                start.elapsed()
            };
            // 254 hover levels (the leaf at NBT depth 509, its siblings at
            // 511) over 12 000 siblings, about 2 MB of accounted NBT, cost
            // about what one level over the same siblings costs (cloning
            // each level's hover event made it 254 times as much)
            let shallow = parse(1);
            let deep = parse(254);
            assert!(
                deep < shallow * 5 + std::time::Duration::from_millis(50)
                    && deep < std::time::Duration::from_secs(1),
                "1 level {shallow:?}, 254 levels {deep:?}"
            );
        });
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn long_text_with_legacy_codes_is_linear() {
        // the longest NBT string (65 535 bytes), with legacy formatting codes
        let text = "\u{a7}cab".repeat(65535 / 5);
        let mut buf = vec![8u8];
        buf.extend_from_slice(&(text.len() as u16).to_be_bytes());
        buf.extend_from_slice(text.as_bytes());
        let start = std::time::Instant::now();
        let c = FormattedText::azalea_read(&mut Cursor::new(&buf[..])).unwrap();
        let plain = c.to_string();
        let elapsed = start.elapsed();
        assert_eq!(plain.len(), 2 * (65535 / 5));
        assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn display_of_deep_chains_is_linear() {
        on_bot_thread(|| {
            // 255 levels of `extra` over 40 000 siblings (about what 2 MB
            // of NBT holds), and a 255-level translate chain over 60 000
            // characters
            let mut c = TextComponent::new("leaf");
            for _ in 0..40_000 {
                c.base.siblings.push(FormattedText::from("a"));
            }
            let mut c = FormattedText::Text(c);
            for _ in 0..255 {
                let mut parent = TextComponent::new("x");
                parent.base.siblings.push(c);
                c = FormattedText::Text(parent);
            }
            let mut t = FormattedText::from("b".repeat(60_000));
            for _ in 0..255 {
                t = FormattedText::Translatable(TranslatableComponent::new(
                    "chat.square_brackets".to_owned(),
                    vec![PrimitiveOrComponent::FormattedText(t)],
                ));
            }
            let start = std::time::Instant::now();
            let a = c.to_string();
            let b = t.to_string();
            let elapsed = start.elapsed();
            assert_eq!(a.len(), 255 + 4 + 40_000);
            assert!(b.len() > 60_000);
            assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
        });
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn rendering_under_a_big_hover_is_linear() {
        use crate::hover_event::HoverEvent;
        on_bot_thread(|| {
            // a component whose hover text has 6000 parts, with 6000
            // siblings: inheriting the hover event cloned it for each sibling
            let mut hover = TextComponent::new("h");
            for _ in 0..6000 {
                hover.base.siblings.push(FormattedText::from("a"));
            }
            let mut c = TextComponent::new("x");
            c.base.style = Box::new(Style {
                hover_event: Some(HoverEvent::ShowText {
                    value: Box::new(FormattedText::Text(hover)),
                }),
                ..Style::default()
            });
            for _ in 0..6000 {
                c.base.siblings.push(FormattedText::from("b"));
            }
            let c = FormattedText::Text(c);
            let start = std::time::Instant::now();
            let ansi = c.to_ansi();
            let html = c.to_html();
            let elapsed = start.elapsed();
            assert!(ansi.contains('b') && html.contains('b'));
            assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
        });
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn nbt_translatable_keeps_extra_and_fallback() {
        let mut c = TranslatableComponent::with_fallback(
            "a.key".to_owned(),
            Some("fallback %s".to_owned()),
            vec![PrimitiveOrComponent::Integer(3)],
        );
        c.base.siblings.push(FormattedText::from("\n"));
        c.base
            .siblings
            .push(FormattedText::Translatable(TranslatableComponent::new(
                "b.key".to_owned(),
                vec![],
            )));
        let c = FormattedText::Translatable(c);
        let (_, back) = round_trip(&c);
        assert_eq!(back, c);
    }
}
