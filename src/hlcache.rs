//! Intern highlight attribute sets into small integer ids.
//!
//! A given attribute set is defined once with an `hl_attr` event and referenced
//! by id forever after, for the lifetime of the connection. Id 0 is the
//! implicit default highlight (matching Neovim) and is never emitted — the host
//! learns its colors from `default_colors`.

use std::collections::HashMap;

use crate::protocol::{Attrs, Event};

/// Connection-lifetime cache mapping attribute sets to highlight ids.
pub struct HlCache {
    map: HashMap<Attrs, u32>,
    next: u32,
}

impl Default for HlCache {
    fn default() -> Self {
        let mut map = HashMap::new();
        map.insert(Attrs::default(), 0);
        Self { map, next: 1 }
    }
}

impl HlCache {
    /// Intern `attrs` to an id, pushing an `hl_attr` event the first time a new
    /// attribute set is seen. Already-defined ids are never re-emitted.
    pub fn intern(&mut self, attrs: &Attrs, out: &mut Vec<Event>) -> u32 {
        if let Some(&id) = self.map.get(attrs) {
            return id;
        }
        let id = self.next;
        self.next += 1;
        self.map.insert(attrs.clone(), id);
        out.push(Event::HlAttr {
            id,
            attrs: attrs.clone(),
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red() -> Attrs {
        Attrs {
            fg: Some(0xff0000),
            ..Attrs::default()
        }
    }

    #[test]
    fn default_attrs_intern_to_zero_without_emitting() {
        let mut cache = HlCache::default();
        let mut out = Vec::new();
        assert_eq!(cache.intern(&Attrs::default(), &mut out), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn first_new_attrs_emit_hl_attr() {
        let mut cache = HlCache::default();
        let mut out = Vec::new();
        let id = cache.intern(&red(), &mut out);
        assert_eq!(id, 1);
        assert_eq!(
            out,
            vec![Event::HlAttr {
                id: 1,
                attrs: red()
            }]
        );
    }

    #[test]
    fn repeated_attrs_reuse_id_without_re_emitting() {
        let mut cache = HlCache::default();
        let mut out = Vec::new();
        let first = cache.intern(&red(), &mut out);
        out.clear();
        let second = cache.intern(&red(), &mut out);
        assert_eq!(first, second);
        assert!(out.is_empty());
    }

    #[test]
    fn distinct_attrs_get_distinct_ids() {
        let mut cache = HlCache::default();
        let mut out = Vec::new();
        let bold = Attrs {
            bold: true,
            ..Attrs::default()
        };
        assert_eq!(cache.intern(&red(), &mut out), 1);
        assert_eq!(cache.intern(&bold, &mut out), 2);
    }
}
