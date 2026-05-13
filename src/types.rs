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
    /// Historia nawigacji (max 20 wpisów) - dla go_back()
    pub history: Vec<String>,
}

impl PageState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self, url: &str) {
        if !self.url.is_empty() && self.url != url {
            self.history.push(self.url.clone());
            if self.history.len() > 20 {
                self.history.remove(0);
            }
        }
        self.url = url.to_string();
        self.title.clear();
    }

    /// Cofnij do poprzedniego URL. Zwraca URL do którego wracamy, lub None jeśli historia pusta.
    pub fn pop_history(&mut self) -> Option<String> {
        if let Some(prev) = self.history.pop() {
            self.url = prev.clone();
            self.title.clear();
            Some(prev)
        } else {
            None
        }
    }
}
