//! 縦書きの描画部品(吹き出し・ト書き)と最終合成のための描画クレート。

pub mod assets;
pub mod bubbles;
pub mod draw;
pub mod font;
pub mod kinsoku;
pub mod layout;

/// 重ねる要素の種類(吹き出し・ト書き)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Bubble,
    Note,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_kind_serde_round_trip() {
        assert_eq!(
            serde_json::to_string(&ItemKind::Bubble).unwrap(),
            "\"bubble\""
        );
        assert_eq!(serde_json::to_string(&ItemKind::Note).unwrap(), "\"note\"");
        assert_eq!(
            serde_json::from_str::<ItemKind>("\"bubble\"").unwrap(),
            ItemKind::Bubble
        );
        assert_eq!(
            serde_json::from_str::<ItemKind>("\"note\"").unwrap(),
            ItemKind::Note
        );
        assert!(serde_json::from_str::<ItemKind>("\"Bubble\"").is_err());
    }
}
