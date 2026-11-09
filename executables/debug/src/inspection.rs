//! Byte-budgeted tree responses. Preserve a depth-first prefix with valid parents.
use alloc::vec::Vec;
use serde_json::{Value, json};

pub(super) fn encode(snapshot: xila::graphics::UiTreeSnapshot) -> Value {
    let mut elements = Vec::new();
    let mut used = 128; // envelope, commas and truncation metadata
    let mut truncated = snapshot.truncated;
    for e in snapshot.elements {
        let buttons: Vec<_> = e.buttons.into_iter().map(|b|json!({
            "index":b.index,"text":b.text,"text_truncated":b.text_truncated,"bounds":b.bounds,
            "hidden":b.hidden,"disabled":b.disabled,"checked":b.checked,
        })).collect();
        let element = json!({
            "id":e.id,"parent_id":e.parent_id,"role":e.role,"semantic_role":e.semantic_role,
            "name":e.name,"text":e.text,"hidden":e.hidden,"disabled":e.disabled,"focused":e.focused,
            "clickable":e.clickable,"checked":e.checked,"bounds":e.bounds,"visible":e.visible,
            "clipped_bounds":e.clipped_bounds,"layer":e.layer,"redacted":e.redacted,
            "text_truncated":e.text_truncated,"name_truncated":e.name_truncated,
            "state":e.state,"scroll":e.scroll,"value":e.value,"range":e.range,"selected":e.selected,
            "selected_text":e.selected_text,"selected_text_truncated":e.selected_text_truncated,
            "buttons":buttons,"buttons_truncated":e.buttons_truncated,
            "style":{"part":"main","background":e.style.background,"text_color":e.style.text_color,
                "border_color":e.style.border_color,"opacity":e.style.opacity,
                "background_opacity":e.style.background_opacity,"border_width":e.style.border_width,
                "radius":e.style.radius,"padding":e.style.padding,"gap":e.style.gap,
                "layout":e.style.layout,"font_line_height":e.style.font_line_height}
        });
        let bytes = serde_json::to_vec(&element)
            .map_or(debug_protocol::MAX_RESPONSE_BYTES, |v| v.len() + 1);
        if used + bytes >= debug_protocol::MAX_RESPONSE_BYTES {
            truncated = true;
            break;
        }
        used += bytes;
        elements.push(element);
    }
    json!({"elements":elements,"truncated":truncated})
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use xila::graphics::{UiElement, UiStyle, UiTreeSnapshot};

    #[test]
    fn escaped_text_is_budgeted_without_losing_a_usable_tree_prefix() {
        let template = UiElement {
            id: 0,
            parent_id: None,
            role: String::from("lv_label"),
            semantic_role: Some("text"),
            name: None,
            text: Some(String::from("\\\"").repeat(256)),
            hidden: false,
            disabled: false,
            focused: false,
            clickable: false,
            checked: None,
            bounds: (0, 0, 100, 20),
            visible: true,
            clipped_bounds: Some((0, 0, 100, 20)),
            layer: "screen",
            redacted: false,
            text_truncated: true,
            name_truncated: false,
            state: 0,
            scroll: (0, 0),
            value: None,
            range: None,
            selected: None,
            selected_text: None,
            selected_text_truncated: false,
            buttons: Vec::new(),
            buttons_truncated: false,
            style: UiStyle {
                background: 0,
                text_color: 0,
                border_color: 0,
                opacity: 255,
                background_opacity: 255,
                border_width: 0,
                radius: 0,
                padding: (0, 0, 0, 0),
                gap: (0, 0),
                layout: 0,
                font_line_height: 16,
            },
        };
        let elements = (0..256)
            .map(|id| {
                let mut e = template.clone();
                e.id = id;
                e
            })
            .collect();
        let value = encode(UiTreeSnapshot {
            elements,
            truncated: false,
        });
        assert_eq!(value["truncated"], true);
        let elements = value["elements"].as_array().unwrap();
        assert!(!elements.is_empty());
        assert!(elements.len() < 256);
        let envelope = json!({"ok":true,"result":value});
        assert!(
            serde_json::to_vec(&envelope).unwrap().len() + 1 < debug_protocol::MAX_RESPONSE_BYTES
        );
    }
}
