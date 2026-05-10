/// Stan sesji przeglądarki.
///
/// Uproszczony - elementy DOM są numerowane przez JS extractor w Chrome,
/// więc nie trzeba ich śledzić po stronie Rust.
/// Trzymamy tylko lokalizację (URL) i tytuł dla kontekstu agenta.

#[derive(Debug, Default)]
pub struct PageState {
    /// Bieżący URL (po redirect)
    pub url: String,
    /// Tytuł strony
    pub title: String,
}

impl PageState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self, url: &str) {
        self.url = url.to_string();
        self.title.clear();
    }
}
