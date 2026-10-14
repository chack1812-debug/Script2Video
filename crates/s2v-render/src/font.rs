//! 縦書き用フォントの読み込みと検査。

use rustybuzz::ttf_parser::{self, Tag};
use rustybuzz::Feature;
use std::path::Path;
use std::sync::Arc;

/// フォントが縦書きの字形置換に使う機能。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalFeature {
    Vrt2,
    Vert,
}

/// 読み込んで検査済みのフォントのフェイス。
/// 自己参照を避けるため、バイト列を持ち、呼び出し側が都度 `rustybuzz::Face` を作る。
#[derive(Debug, Clone)]
pub struct FontFace {
    data: Arc<Vec<u8>>,
    index: u32,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub vertical: VerticalFeature,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FontError {
    Read(String),
    BadIndex {
        index: u32,
        count: u32,
        names: Vec<String>,
    },
    Parse(String),
    NoVerticalMetrics,
    NoVerticalFeature,
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::Read(e) => write!(f, "フォントファイルを読めません: {e}"),
            FontError::BadIndex {
                index,
                count,
                names,
            } => {
                write!(f, "フォントの番号 {index} は範囲外です。このファイルには {count} 個のフェイスが入っています")?;
                if !names.is_empty() {
                    let list: Vec<String> = names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| {
                            format!("{i}: {}", if n.is_empty() { "(名前なし)" } else { n })
                        })
                        .collect();
                    write!(f, "({})", list.join("、"))?;
                }
                Ok(())
            }
            FontError::Parse(e) => write!(f, "フォントを解析できません: {e}"),
            FontError::NoVerticalMetrics => {
                write!(f, "このフォントには縦書きの寸法(vhea・vmtx)がありません。縦書きに対応したフォントを指定してください")
            }
            FontError::NoVerticalFeature => {
                write!(f, "このフォントには縦書きの字形置換(vrt2・vert)がありません。縦書きに対応したフォントを指定してください")
            }
        }
    }
}

impl std::error::Error for FontError {}

fn face_name(data: &[u8], index: u32) -> String {
    let Ok(face) = ttf_parser::Face::parse(data, index) else {
        return String::new();
    };
    face.names()
        .into_iter()
        .find(|n| n.name_id == ttf_parser::name_id::FULL_NAME)
        .and_then(|n| n.to_string())
        .unwrap_or_default()
}

impl FontFace {
    pub fn load(path: &Path, index: u32) -> Result<FontFace, FontError> {
        let data =
            std::fs::read(path).map_err(|e| FontError::Read(format!("{}: {e}", path.display())))?;
        FontFace::from_bytes(data, index)
    }

    pub fn from_bytes(data: Vec<u8>, index: u32) -> Result<FontFace, FontError> {
        let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
        if index >= count {
            let names = (0..count).map(|i| face_name(&data, i)).collect();
            return Err(FontError::BadIndex {
                index,
                count,
                names,
            });
        }
        let (units_per_em, ascender, descender, vertical) = {
            let face = ttf_parser::Face::parse(&data, index)
                .map_err(|e| FontError::Parse(e.to_string()))?;
            let t = face.tables();
            if t.vhea.is_none() || t.vmtx.is_none() {
                return Err(FontError::NoVerticalMetrics);
            }
            let mut has_vrt2 = false;
            let mut has_vert = false;
            if let Some(gsub) = t.gsub {
                for i in 0..gsub.features.len() {
                    if let Some(feat) = gsub.features.get(i) {
                        has_vrt2 |= feat.tag == Tag::from_bytes(b"vrt2");
                        has_vert |= feat.tag == Tag::from_bytes(b"vert");
                    }
                }
            }
            let vertical = if has_vrt2 {
                VerticalFeature::Vrt2
            } else if has_vert {
                VerticalFeature::Vert
            } else {
                return Err(FontError::NoVerticalFeature);
            };
            (
                face.units_per_em(),
                face.ascender(),
                face.descender(),
                vertical,
            )
        };
        Ok(FontFace {
            data: Arc::new(data),
            index,
            units_per_em,
            ascender,
            descender,
            vertical,
        })
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    /// rustybuzz に渡す縦書きの機能(Vrt2 なら [-vert, +vrt2]、Vert なら [+vert])
    pub fn vertical_features(&self) -> Vec<Feature> {
        match self.vertical {
            VerticalFeature::Vrt2 => vec![
                Feature::new(Tag::from_bytes(b"vert"), 0, ..),
                Feature::new(Tag::from_bytes(b"vrt2"), 1, ..),
            ],
            VerticalFeature::Vert => vec![Feature::new(Tag::from_bytes(b"vert"), 1, ..)],
        }
    }

    /// text の文字のうち、フォントに字形が無いもの(重複なし、出現順)。
    /// ASCII の制御文字・空白(U+0020)・全角空白(U+3000)は対象外。
    pub fn missing_chars(&self, text: &str) -> Vec<char> {
        let Ok(face) = ttf_parser::Face::parse(&self.data, self.index) else {
            return Vec::new();
        };
        let mut out: Vec<char> = Vec::new();
        for c in text.chars() {
            if c.is_ascii_control() || c == ' ' || c == '\u{3000}' {
                continue;
            }
            if face.glyph_index(c).is_none() && !out.contains(&c) {
                out.push(c);
            }
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) fn font_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fonts")
            .join(name)
    }

    #[test]
    fn vrt2_is_preferred_and_vert_is_disabled() {
        let f = FontFace::load(&font_path("test-vrt2.ttf"), 0).unwrap();
        assert_eq!(f.vertical, VerticalFeature::Vrt2);
        let feats: Vec<String> = f
            .vertical_features()
            .iter()
            .map(|x| format!("{x:?}"))
            .collect();
        // vert は値 0(無効)、vrt2 は値 1(有効)
        assert_eq!(f.vertical_features().len(), 2, "{feats:?}");
        assert!(f
            .vertical_features()
            .iter()
            .any(|x| x.tag == rustybuzz::ttf_parser::Tag::from_bytes(b"vert") && x.value == 0));
        assert!(f
            .vertical_features()
            .iter()
            .any(|x| x.tag == rustybuzz::ttf_parser::Tag::from_bytes(b"vrt2") && x.value == 1));
    }

    #[test]
    fn vert_only_font_uses_vert() {
        let f = FontFace::load(&font_path("test-vert.ttf"), 0).unwrap();
        assert_eq!(f.vertical, VerticalFeature::Vert);
        let v = f.vertical_features();
        assert_eq!(v.len(), 1);
        assert_eq!(
            (v[0].tag, v[0].value),
            (rustybuzz::ttf_parser::Tag::from_bytes(b"vert"), 1)
        );
    }

    #[test]
    fn font_without_vertical_feature_is_rejected() {
        assert_eq!(
            FontFace::load(&font_path("test-novert.ttf"), 0).err(),
            Some(FontError::NoVerticalFeature)
        );
    }

    #[test]
    fn font_without_vhea_is_rejected() {
        assert_eq!(
            FontFace::load(&font_path("test-novhea.ttf"), 0).err(),
            Some(FontError::NoVerticalMetrics)
        );
    }

    #[test]
    fn font_without_vmtx_is_rejected() {
        assert_eq!(
            FontFace::load(&font_path("test-novmtx.ttf"), 0).err(),
            Some(FontError::NoVerticalMetrics)
        );
    }

    #[test]
    fn bad_index_and_missing_file() {
        let e = FontFace::load(&font_path("test-vrt2.ttf"), 3)
            .err()
            .unwrap();
        assert!(
            matches!(
                e,
                FontError::BadIndex {
                    index: 3,
                    count: 1,
                    ..
                }
            ),
            "{e:?}"
        );
        assert!(e.to_string().contains("1"), "{e}");
        assert!(matches!(
            FontFace::load(&font_path("無い.ttf"), 0),
            Err(FontError::Read(_))
        ));
        assert!(matches!(
            FontFace::from_bytes(b"not a font".to_vec(), 0),
            Err(FontError::Parse(_))
        ));
    }

    #[test]
    fn missing_chars_lists_each_once_in_order() {
        let f = FontFace::load(&font_path("test-vrt2.ttf"), 0).unwrap();
        assert_eq!(f.missing_chars("港ゑ朝😀港😀龘"), vec!['ゑ', '😀', '龘']); // ゑ・龘・絵文字は部分集合に無い
        assert!(f.missing_chars("港、朝。").is_empty());
    }

    #[test]
    fn system_yu_gothic_is_accepted_when_present() {
        let p = std::path::Path::new("C:\\Windows\\Fonts\\YuGothM.ttc");
        if !p.exists() {
            return; // Windows 以外・游ゴシックが無い環境では確かめない
        }
        let f = FontFace::load(p, 0).unwrap();
        assert_eq!(f.vertical, VerticalFeature::Vrt2);
    }
}
