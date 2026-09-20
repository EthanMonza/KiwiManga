//! Auto-detection of the user's language via whatlang.
//! A manual /language choice always overrides this guess.
//!
//! Note: whatlang has no Maori (mi) support, so `mi` is selectable only
//! manually via /language. Unknown/unsupported detections -> None (caller
//! keeps the previous locale).

use whatlang::Lang;

/// Minimum confidence to trust a guess. Titles are short, so the bar is low;
/// a wrong guess only affects the default chapter language, never blocks.
const MIN_CONFIDENCE: f64 = 0.30;

pub fn detect_locale(text: &str) -> Option<&'static str> {
    if text.trim().chars().count() < 2 {
        return None;
    }
    let info = whatlang::detect(text)?;
    if info.confidence() < MIN_CONFIDENCE {
        return None;
    }
    lang_to_locale(info.lang())
}

pub fn lang_to_locale(lang: Lang) -> Option<&'static str> {
    let code = match lang {
        Lang::Eng => "en",
        Lang::Rus => "ru",
        Lang::Tur => "tr",
        Lang::Deu => "de",
        Lang::Dan => "da",
        Lang::Nob => "no",
        Lang::Swe => "sv",
        Lang::Fra => "fr",
        Lang::Ara => "ar",
        Lang::Jpn => "ja",
        Lang::Kor => "ko",
        Lang::Cmn => "zh",
        Lang::Fin => "fi",
        Lang::Spa => "es",
        Lang::Ita => "it",
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_core_languages() {
        assert_eq!(lang_to_locale(Lang::Rus), Some("ru"));
        assert_eq!(lang_to_locale(Lang::Eng), Some("en"));
        assert_eq!(lang_to_locale(Lang::Tur), Some("tr"));
        assert_eq!(lang_to_locale(Lang::Epo), None);
    }

    #[test]
    fn detects_russian_sentence() {
        let loc = detect_locale("Привет, как скачать последнюю главу Берсерка?");
        assert_eq!(loc, Some("ru"));
    }

    #[test]
    fn short_text_is_none() {
        assert_eq!(detect_locale("x"), None);
    }
}
