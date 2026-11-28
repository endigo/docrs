//! Font sets the caller loads once and passes to conversions by registry
//! id. The registry lives in the instance's memory: `js/index.js` keeps the
//! files and registers them again on a fresh instance.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use docrs::Error;
use docrs::layout::{FontInfo, FontSet};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::{js_error, js_length, profile, set_fields, to_js};

/// Font budgets of a `limits` profile.
#[derive(Debug, Clone, Copy)]
pub struct FontLimits {
    pub file_bytes: u64,
    pub set_bytes: u64,
    pub faces: u64,
    /// Sets one instance holds at once.
    pub sets: u64,
}

impl FontLimits {
    pub const DEFAULT: FontLimits = FontLimits {
        file_bytes: 32 << 20,
        set_bytes: 64 << 20,
        faces: 64,
        sets: 64,
    };
    pub const SERVERLESS: FontLimits = FontLimits {
        file_bytes: 8 << 20,
        set_bytes: 16 << 20,
        faces: 24,
        sets: 8,
    };

    /// Refuse files by their sizes alone, before any is read.
    fn check_files(&self, sizes: &[u64]) -> Result<(), Error> {
        let count = sizes.len() as u64;
        if count > self.faces {
            return Err(Error::limit("max_font_faces", self.faces, count));
        }
        self.check(
            sizes.iter().copied().max().unwrap_or(0),
            sizes.iter().copied().fold(0u64, u64::saturating_add),
            0,
        )
    }

    fn check(&self, largest: u64, bytes: u64, faces: u64) -> Result<(), Error> {
        if largest > self.file_bytes {
            return Err(Error::limit(
                "max_font_file_bytes",
                self.file_bytes,
                largest,
            ));
        }
        if bytes > self.set_bytes {
            return Err(Error::limit("max_font_set_bytes", self.set_bytes, bytes));
        }
        if faces > self.faces {
            return Err(Error::limit("max_font_faces", self.faces, faces));
        }
        Ok(())
    }
}

/// A font set and the sizes of the files it was built from, which every
/// call checks against its own profile.
#[derive(Debug, Clone, Default)]
pub struct LoadedFonts {
    pub set: FontSet,
    largest: u64,
    bytes: u64,
}

impl LoadedFonts {
    pub fn check(&self, limits: &FontLimits) -> Result<(), Error> {
        limits.check(self.largest, self.bytes, self.set.faces().len() as u64)
    }
}

/// Options of `loadFonts`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct FontOptions {
    pub limits: Option<String>,
    /// Family a document names → family of the set to lay it out with.
    pub aliases: BTreeMap<String, String>,
}

/// Build a set from `files`, refusing them all when one cannot be read
/// (the error names its index).
pub fn load(
    files: impl Iterator<Item = Vec<u8>>,
    sizes: &[u64],
    options: &FontOptions,
) -> Result<LoadedFonts, (Error, Option<usize>)> {
    let limits = profile(options.limits.as_deref())
        .map_err(|err| (err, None))?
        .1
        .fonts;
    limits.check_files(sizes).map_err(|err| (err, None))?;
    let mut builder = FontSet::builder();
    for (at, file) in files.enumerate() {
        builder
            .add(file)
            .map_err(|err| (err.context(format!("fonts[{at}]")), Some(at)))?;
    }
    for (family, to) in &options.aliases {
        builder.alias(family, to);
    }
    let loaded = LoadedFonts {
        set: builder.build(),
        largest: sizes.iter().copied().max().unwrap_or(0),
        bytes: sizes.iter().copied().fold(0u64, u64::saturating_add),
    };
    loaded.check(&limits).map_err(|err| (err, None))?;
    Ok(loaded)
}

#[derive(Default)]
struct Registry {
    sets: HashMap<u32, LoadedFonts>,
    next: u32,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::default();
}

fn register(loaded: LoadedFonts, limits: &FontLimits) -> Result<u32, Error> {
    REGISTRY.with_borrow_mut(|registry| {
        let live = registry.sets.len() as u64;
        if live >= limits.sets {
            return Err(Error::limit("max_font_sets", limits.sets, live + 1));
        }
        registry.next = registry.next.wrapping_add(1);
        let id = registry.next;
        registry.sets.insert(id, loaded);
        Ok(id)
    })
}

/// The set registered as `id`.
pub fn registered(id: u32) -> Result<LoadedFonts, Error> {
    REGISTRY
        .with_borrow(|registry| registry.sets.get(&id).cloned())
        .ok_or_else(|| {
            Error::invalid_argument(
                "font_set_unknown",
                format!("fonts: no font set is registered as {id} in this instance"),
            )
        })
}

/// The registry id `options.fonts` gives, if any.
pub fn id_of(value: &JsValue) -> Result<u32, Error> {
    value
        .as_f64()
        .filter(|id| id.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(id))
        .map(|id| id as u32)
        .ok_or_else(|| {
            Error::invalid_argument(
                "invalid_options",
                "fonts must be a font set from loadFonts or loadFontManifest",
            )
        })
}

fn faces_to_js(faces: &[FontInfo]) -> JsValue {
    to_js(&serde_json::Value::Array(
        faces
            .iter()
            .map(|face| {
                serde_json::json!({
                    "family": face.family,
                    "bold": face.bold,
                    "italic": face.italic,
                    "variable": face.variable,
                    "cff": face.cff,
                    "embeddable": face.embeddable,
                    "subsettable": face.subsettable,
                })
            })
            .collect(),
    ))
}

/// `loadFonts` options, refusing unknown ones (serde-wasm-bindgen reads only
/// the fields it knows).
fn font_options(options: Option<js_sys::Object>) -> Result<FontOptions, Error> {
    let Some(options) = options else {
        return Ok(FontOptions::default());
    };
    let given = set_fields(&options);
    for key in js_sys::Object::keys(&given).iter() {
        let name = key.as_string().unwrap_or_default();
        if !matches!(name.as_str(), "limits" | "aliases") {
            return Err(Error::invalid_argument(
                "invalid_options",
                format!("unknown font option {name:?}"),
            ));
        }
    }
    serde_wasm_bindgen::from_value(given.into())
        .map_err(|err| Error::invalid_argument("invalid_options", format!("font options: {err}")))
}

/// Register `files` (an array of `Uint8Array`) as one font set: `{ id,
/// faces }`. Internal to `js/index.js`, which wraps it as `loadFonts`.
#[wasm_bindgen(js_name = registerFonts, skip_typescript)]
pub fn register_fonts(files: JsValue, options: Option<js_sys::Object>) -> Result<JsValue, JsValue> {
    let files: Vec<js_sys::Uint8Array> = files
        .dyn_into::<js_sys::Array>()
        .ok()
        .and_then(|files| files.iter().map(|file| file.dyn_into().ok()).collect())
        .ok_or_else(|| {
            js_error(&Error::invalid_argument(
                "invalid_argument",
                "files must be an array of Uint8Array",
            ))
        })?;
    let options = font_options(options).map_err(|err| js_error(&err))?;
    let sizes: Vec<u64> = files.iter().map(|file| js_length(file)).collect();
    let limits = profile(options.limits.as_deref())
        .map_err(|err| js_error(&err))?
        .1
        .fonts;
    let loaded =
        load(files.iter().map(|file| file.to_vec()), &sizes, &options).map_err(|(err, file)| {
            let js = js_error(&err);
            if let Some(file) = file {
                let _ = js_sys::Reflect::set(&js, &"file".into(), &(file as u32).into());
            }
            js
        })?;
    let faces = faces_to_js(loaded.set.faces());
    let id = register(loaded, &limits).map_err(|err| js_error(&err))?;
    let result = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&result, &"id".into(), &id.into());
    let _ = js_sys::Reflect::set(&result, &"faces".into(), &faces);
    Ok(result.into())
}

/// The font budgets of a `limits` profile, for `loadFontManifest`'s caps
/// on what it fetches. Internal to `js/index.js`.
#[wasm_bindgen(js_name = fontLimits, skip_typescript)]
pub fn font_limits(limits: Option<String>) -> Result<JsValue, JsValue> {
    let limits = profile(limits.as_deref())
        .map_err(|err| js_error(&err))?
        .1
        .fonts;
    Ok(to_js(&serde_json::json!({
        "fileBytes": limits.file_bytes,
        "setBytes": limits.set_bytes,
    })))
}

/// Release the set registered as `id`; whether it was registered.
#[wasm_bindgen(js_name = freeFonts, skip_typescript)]
pub fn free_fonts(id: u32) -> bool {
    REGISTRY.with_borrow_mut(|registry| registry.sets.remove(&id).is_some())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn font(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/fonts")
                .join(name),
        )
        .unwrap()
    }

    fn load_all(
        files: Vec<Vec<u8>>,
        limits: Option<&str>,
    ) -> Result<LoadedFonts, (Error, Option<usize>)> {
        let sizes: Vec<u64> = files.iter().map(|file| file.len() as u64).collect();
        let options = FontOptions {
            limits: limits.map(String::from),
            ..FontOptions::default()
        };
        load(files.into_iter(), &sizes, &options)
    }

    #[test]
    fn one_bad_file_refuses_the_set_and_names_it() {
        let serif = font("liberation/LiberationSerif-Regular.ttf");
        let (err, file) = load_all(vec![serif.clone(), b"not a font".to_vec()], None).unwrap_err();
        assert_eq!(file, Some(1));
        assert_eq!(err.reason(), "font_format_unsupported");
        assert!(err.message().starts_with("fonts[1]: "), "{err}");
        let loaded = load_all(vec![serif], None).unwrap();
        assert_eq!(loaded.set.faces()[0].family, "Liberation Serif");
    }

    #[test]
    fn a_set_loaded_under_default_budgets_is_checked_against_each_call_s() {
        let serif = font("liberation/LiberationSerif-Regular.ttf");
        let loaded = load_all(vec![serif.clone(); 25], None).unwrap();
        assert!(loaded.check(&FontLimits::DEFAULT).is_ok());
        let err = loaded.check(&FontLimits::SERVERLESS).unwrap_err();
        assert_eq!(err.reason(), "max_font_faces");
        let (err, file) = load_all(vec![serif; 25], Some("serverless")).unwrap_err();
        assert_eq!((err.reason(), file), ("max_font_faces", None));
    }

    #[test]
    fn sizes_are_refused_before_the_files_are_read() {
        let sizes = [(8 << 20) + 1];
        let options = FontOptions {
            limits: Some("serverless".into()),
            ..FontOptions::default()
        };
        let (err, _) = load(std::iter::empty(), &sizes, &options).unwrap_err();
        assert_eq!(err.reason(), "max_font_file_bytes");
        let (err, _) = load(std::iter::empty(), &[6 << 20; 3], &options).unwrap_err();
        assert_eq!(err.reason(), "max_font_set_bytes");
    }

    #[test]
    fn the_registry_caps_live_sets_and_frees_by_id() {
        let limits = FontLimits {
            sets: 2,
            ..FontLimits::SERVERLESS
        };
        let first = register(LoadedFonts::default(), &limits).unwrap();
        let second = register(LoadedFonts::default(), &limits).unwrap();
        assert_ne!(first, second);
        let err = register(LoadedFonts::default(), &limits).unwrap_err();
        assert_eq!(err.reason(), "max_font_sets");
        assert!(free_fonts(first));
        assert!(!free_fonts(first));
        assert_eq!(registered(first).unwrap_err().reason(), "font_set_unknown");
        assert!(registered(second).is_ok());
        assert!(register(LoadedFonts::default(), &limits).is_ok());
    }
}
