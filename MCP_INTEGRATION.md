 # Integracja z serwerami MCP

Ten dokument opisuje, jak korzystać z różnych serwerów MCP w ramach projektu rust-browser do wspierania rozwoju oprogramowania.

## Dostępne serwery MCP

### 1. aisquare-playwright
- **Opis**: Serwer do automatyzacji przeglądarek przy użyciu Playwright
- **Zastosowanie**: Testowanie funkcjonalne, automatyzacja zadań w przeglądarce
- **Integracja z rust-browser**: Można wykorzystać do porównywania wyników działania rust-browser z innym narzędziem automatyzacji

### 2. memory-bank
- **Opis**: Serwer do przechowywania i zarządzania pamięcią kontekstową
- **Zastosowanie**: Przechowywanie informacji o stanie sesji, danych tymczasowych
- **Integracja z rust-browser**: Możliwość udostępniania danych sesji między sesjami przeglądarki

### 3. mega-filesystem
- **Opis**: Serwer do zaawansowanych operacji na systemie plików
- **Zastosowanie**: Operacje na plikach i katalogach poza standardowymi możliwościami MCP
- **Integracja z rust-browser**: Możliwość zapisywania wyników przeglądania, skryptów, danych sesji

### 4. ultra-brain
- **Opis**: Serwer AI do zaawansowanej analizy i przetwarzania
- **Zastosowanie**: Analiza danych, inteligentne przetwarzanie treści
- **Integracja z rust-browser**: Wsparcie w analizowaniu i interpretowaniu treści stron internetowych

### 5. live-function-tree
- **Opis**: Serwer do dynamicznego generowania i zarządzania drzewami funkcji
- **Zastosowanie**: Organizacja i zarządzanie logiką aplikacji
- **Integracja z rust-browser**: Pomoc w organizacji struktury projektu

### 6. smart-tree
- **Opis**: Inteligentny serwer do analizy struktur danych
- **Zastosowanie**: Analiza i optymalizacja struktur danych
- **Integracja z rust-browser**: Optymalizacja struktur danych w wynikach przetwarzania stron

### 7. vector-database
- **Opis**: Serwer bazodanowy do operacji na wektorach
- **Zastosowanie**: Przetwarzanie danych semantycznych, embeddingi
- **Integracja z rust-browser**: Wsparcie w analizie semantycznej treści stron

### 8. multi-model-mcp
- **Opis**: Serwer do koordynacji wielu modeli AI
- **Zastosowanie**: Koordynacja różnych modeli do wspólnego zadania
- **Integracja z rust-browser**: Wspólne przetwarzanie danych przez wiele modeli

### 9. snapshot-maker
- **Opis**: Serwer do tworzenia migawek stanu
- **Zastosowanie**: Zapisywanie i odtwarzanie stanów systemu
- **Integracja z rust-browser**: Możliwość zapisywania stanów sesji przeglądania

### 10. swarm-high-speed
- **Opis**: Serwer do wieloagentowego przetwarzania
- **Zastosowanie**: Wspólne przetwarzanie zadań przez wiele agentów
- **Integracja z rust-browser**: Wspólne analizowanie stron przez wiele agentów

## Przykładowe scenariusze użycia

### Scenariusz 1: Kompleksowa analiza strony
1. Użycie `rust-browser` do nawigacji i pozyskania struktury strony
2. Przekazanie danych do `ultra-brain` do analizy semantycznej
3. Zapisanie wyników w `memory-bank` do dalszego użycia
4. Przechowanie danych w `mega-filesystem` do analizy offline

### Scenariusz 2: Testowanie funkcjonalności
1. Użycie `rust-browser` do interakcji z testowaną stroną
2. Równoległe testy z użyciem `aisquare-playwright` do walidacji wyników
3. Przechowywanie wyników testów w `mega-filesystem`
4. Analiza różnic przez `ultra-brain`

### Scenariusz 3: Przetwarzanie semantyczne
1. Pozyskanie treści strony przez `rust-browser`
2. Konwersja do wektorów przez `vector-database`
3. Przetworzenie wieloma modelami przez `multi-model-mcp`
4. Zapis wyników w `memory-bank`

## Konfiguracja środowiska

Aby korzystać z tych serwerów MCP, upewnij się, że masz odpowiednią konfigurację w pliku `~/.config/claude/claude_desktop_config.json` lub `.vscode/mcp.json`:

```json
{
  "servers": {
    "aisquare-playwright": {
      "command": "npx",
      "args": ["aisquare-playwright-mcp@latest"],
      "type": "stdio"
    },
    "memory-bank": {
      "command": "node",
      "args": ["build/index.js"],
      "cwd": "C:\\mcp-servers\\mcp-memory-server",
      "type": "stdio"
    },
    "rust-browser": {
      "command": "C:\\rust-browser\\target\\release\\rust-browser-mcp.exe",
      "cwd": "C:\\rust-browser",
      "env": {
        "RUST_LOG": "info"
      },
      "type": "stdio",
      "description": "Rust Chrome MCP browser agent - navigate, screenshot (vision), get_text, get_links, fill_and_submit"
    }
  }
}
```

## Przykładowe wywołania narzędzi

Po skonfigurowaniu wszystkich serwerów, możesz używać ich sekwencyjnie lub równolegle w ramach jednego przepływu pracy:

```
rust-browser/navigate(url="https://example.com")
ultra-brain/analyze(content=result_from_rust_browser)
vector-database/store(data=analysis_result)
memory-bank/save(key="example_analysis", value=stored_data)
```

W ten sposób możesz tworzyć zaawansowane przepływy pracy łączące możliwości różnych serwerów MCP.
