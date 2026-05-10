/// Captcha solver - integracja z zewnętrznymi serwisami
///
/// Obsługiwane serwisy (kompatybilne API, taki sam format):
///   - 2captcha.com       (najpopularniejszy, ~$1.5/1000)
///   - anti-captcha.com   (nieco droższy, szybszy)
///   - CapMonster Cloud   (tańszy, API kompatybilne z anti-captcha)
///
/// Obsługiwane typy captchy:
///
/// 1. IMAGE captcha (stary typ - "wpisz znaki z obrazka")
///    - Bierzemy screenshot elementu captchy z Chrome
///    - Wysyłamy base64 PNG do serwisu
///    - Dostajemy string z odpowiedzią
///
/// 2. reCAPTCHA v2 / v3 (Google "Nie jestem robotem")
///    - Nie wysyłamy obrazka - serwis potrzebuje tylko site_key + URL
///    - Serwis rozwiązuje po swojemu (ma farmy tokenów)
///    - Dostajemy g-recaptcha-response token
///    - Wstrzykujemy token przez JS do formularza i submitujemy
///
/// 3. hCaptcha (Cloudflare, wiele serwisów od 2023+)
///    - Identycznie jak reCAPTCHA v2 ale inny endpoint
///    - Dostajemy h-captcha-response token
///
/// Przepływ dla agenta:
///   1. navigate(url) → WAP-XML pokazuje że jest captcha lub token field
///   2. screenshot()  → agent/Judge widzi jaki typ captchy
///   3. solve_captcha(type, ...) → dostaje token/string
///   4. fill_and_submit() z tokenem → formularz idzie dalej

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Konfiguracja
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CaptchaConfig {
    /// Klucz API serwisu
    pub api_key: String,
    /// Serwis do użycia
    pub service: CaptchaService,
    /// Timeout na odpowiedź (serwis czeka na ludzką odpowiedź)
    pub timeout: Duration,
    /// Interwał polling - jak często pytać "już gotowe?"
    pub poll_interval: Duration,
}

#[derive(Debug, Clone)]
pub enum CaptchaService {
    /// https://2captcha.com - najpopularniejszy
    TwoCaptcha,
    /// https://anti-captcha.com - szybszy, droższy
    AntiCaptcha,
    /// https://capmonster.cloud - najtańszy
    CapMonster,
}

impl CaptchaService {
    /// Bazowy URL API dla serwisu
    fn base_url(&self) -> &str {
        match self {
            // 2captcha używa innego formatu niż anti-captcha
            CaptchaService::TwoCaptcha => "https://2captcha.com",
            // anti-captcha i capmonster mają identyczne API (JSON-RPC style)
            CaptchaService::AntiCaptcha => "https://api.anti-captcha.com",
            CaptchaService::CapMonster => "https://api.capmonster.cloud",
        }
    }
}

impl Default for CaptchaConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            service: CaptchaService::TwoCaptcha,
            timeout: Duration::from_secs(120),
            poll_interval: Duration::from_secs(5),
        }
    }
}

// ---------------------------------------------------------------------------
// Typy captchy
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CaptchaTask {
    /// Stary typ - obrazek z tekstem do przepisania
    /// base64_image: PNG/JPG zakodowany base64
    Image {
        base64_image: String,
    },

    /// Google reCAPTCHA v2 ("Nie jestem robotem" checkbox)
    /// Nie potrzeba obrazka - serwis ma swoje metody
    RecaptchaV2 {
        site_key: String,   // data-sitekey z HTML
        page_url: String,
    },

    /// Google reCAPTCHA v3 (niewidoczna, scoring)
    RecaptchaV3 {
        site_key: String,
        page_url: String,
        action: String,     // np. "login", "register"
        min_score: f32,     // 0.3 = domyślnie, 0.7 = strict
    },

    /// hCaptcha (Cloudflare i inne serwisy)
    HCaptcha {
        site_key: String,
        page_url: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CaptchaSolution {
    /// Rozwiązanie: tekst z obrazka LUB token g-recaptcha-response / h-captcha-response
    pub solution: String,
    /// Typ który był rozwiązywany
    pub captcha_type: String,
    /// ID zadania w serwisie (do ewentualnego reportowania błędów)
    pub task_id: String,
}

// ---------------------------------------------------------------------------
// Solver
// ---------------------------------------------------------------------------

pub struct CaptchaSolver {
    client: reqwest::Client,
    config: CaptchaConfig,
}

impl CaptchaSolver {
    pub fn new(config: CaptchaConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { client, config })
    }

    /// Rozwiązuje captchę - blokuje do czasu aż serwis zwróci odpowiedź.
    pub async fn solve(&self, task: CaptchaTask) -> Result<CaptchaSolution> {
        match &self.config.service {
            CaptchaService::TwoCaptcha => self.solve_2captcha(task).await,
            CaptchaService::AntiCaptcha | CaptchaService::CapMonster => {
                self.solve_anticaptcha(task).await
            }
        }
    }

    // -----------------------------------------------------------------------
    // 2captcha API (stary format: form POST do /in.php, polling /res.php)
    // -----------------------------------------------------------------------

    async fn solve_2captcha(&self, task: CaptchaTask) -> Result<CaptchaSolution> {
        let base = self.config.service.base_url();
        let api_key = &self.config.api_key;

        // Wyślij zadanie
        let (params, captcha_type) = match &task {
            CaptchaTask::Image { base64_image } => {
                (vec![
                    ("method", "base64".to_string()),
                    ("key", api_key.clone()),
                    ("body", base64_image.clone()),
                    ("json", "1".to_string()),
                ], "image")
            }
            CaptchaTask::RecaptchaV2 { site_key, page_url } => {
                (vec![
                    ("method", "userrecaptcha".to_string()),
                    ("key", api_key.clone()),
                    ("googlekey", site_key.clone()),
                    ("pageurl", page_url.clone()),
                    ("json", "1".to_string()),
                ], "recaptcha_v2")
            }
            CaptchaTask::RecaptchaV3 { site_key, page_url, action, min_score } => {
                (vec![
                    ("method", "userrecaptcha".to_string()),
                    ("key", api_key.clone()),
                    ("googlekey", site_key.clone()),
                    ("pageurl", page_url.clone()),
                    ("version", "v3".to_string()),
                    ("action", action.clone()),
                    ("min_score", min_score.to_string()),
                    ("json", "1".to_string()),
                ], "recaptcha_v3")
            }
            CaptchaTask::HCaptcha { site_key, page_url } => {
                (vec![
                    ("method", "hcaptcha".to_string()),
                    ("key", api_key.clone()),
                    ("sitekey", site_key.clone()),
                    ("pageurl", page_url.clone()),
                    ("json", "1".to_string()),
                ], "hcaptcha")
            }
        };

        info!("2captcha: submitting {} task", captcha_type);

        let submit_resp: serde_json::Value = self.client
            .post(format!("{}/in.php", base))
            .form(&params)
            .send()
            .await
            .context("2captcha submit failed")?
            .json()
            .await?;

        if submit_resp["status"].as_i64() != Some(1) {
            anyhow::bail!("2captcha submit error: {}", submit_resp["request"]);
        }

        let task_id = submit_resp["request"]
            .as_str()
            .context("missing task id")?
            .to_string();

        info!("2captcha: task_id={}, polling...", task_id);

        // Polling - czekamy aż serwis rozwiąże
        let deadline = tokio::time::Instant::now() + self.config.timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                anyhow::bail!("2captcha timeout after {:?}", self.config.timeout);
            }

            tokio::time::sleep(self.config.poll_interval).await;

            let poll_resp: serde_json::Value = self.client
                .get(format!("{}/res.php", base))
                .query(&[
                    ("key", api_key.as_str()),
                    ("action", "get"),
                    ("id", task_id.as_str()),
                    ("json", "1"),
                ])
                .send()
                .await?
                .json()
                .await?;

            match poll_resp["status"].as_i64() {
                Some(1) => {
                    let solution = poll_resp["request"]
                        .as_str()
                        .context("missing solution")?
                        .to_string();
                    info!("2captcha: solved! type={}", captcha_type);
                    return Ok(CaptchaSolution {
                        solution,
                        captcha_type: captcha_type.to_string(),
                        task_id,
                    });
                }
                _ => {
                    let status = poll_resp["request"].as_str().unwrap_or("unknown");
                    if status != "CAPCHA_NOT_READY" {
                        anyhow::bail!("2captcha error: {}", status);
                    }
                    // CAPCHA_NOT_READY - czekamy dalej
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // anti-captcha / CapMonster API (JSON zadania, wspólny format)
    // -----------------------------------------------------------------------

    async fn solve_anticaptcha(&self, task: CaptchaTask) -> Result<CaptchaSolution> {
        let base = self.config.service.base_url();
        let api_key = &self.config.api_key;

        // Buduj task object
        let (task_obj, captcha_type) = match &task {
            CaptchaTask::Image { base64_image } => (
                serde_json::json!({
                    "type": "ImageToTextTask",
                    "body": base64_image,
                }),
                "image",
            ),
            CaptchaTask::RecaptchaV2 { site_key, page_url } => (
                serde_json::json!({
                    "type": "NoCaptchaTaskProxyless",
                    "websiteURL": page_url,
                    "websiteKey": site_key,
                }),
                "recaptcha_v2",
            ),
            CaptchaTask::RecaptchaV3 { site_key, page_url, action, min_score } => (
                serde_json::json!({
                    "type": "RecaptchaV3TaskProxyless",
                    "websiteURL": page_url,
                    "websiteKey": site_key,
                    "pageAction": action,
                    "minScore": min_score,
                }),
                "recaptcha_v3",
            ),
            CaptchaTask::HCaptcha { site_key, page_url } => (
                serde_json::json!({
                    "type": "HCaptchaTaskProxyless",
                    "websiteURL": page_url,
                    "websiteKey": site_key,
                }),
                "hcaptcha",
            ),
        };

        info!("anti-captcha: submitting {} task", captcha_type);

        let create_resp: serde_json::Value = self.client
            .post(format!("{}/createTask", base))
            .json(&serde_json::json!({
                "clientKey": api_key,
                "task": task_obj,
            }))
            .send()
            .await
            .context("anti-captcha createTask failed")?
            .json()
            .await?;

        if create_resp["errorId"].as_i64() != Some(0) {
            anyhow::bail!(
                "anti-captcha error: {} - {}",
                create_resp["errorCode"],
                create_resp["errorDescription"]
            );
        }

        let task_id = create_resp["taskId"]
            .as_i64()
            .context("missing taskId")?
            .to_string();

        info!("anti-captcha: taskId={}, polling...", task_id);

        // Polling
        let deadline = tokio::time::Instant::now() + self.config.timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                anyhow::bail!("anti-captcha timeout after {:?}", self.config.timeout);
            }

            tokio::time::sleep(self.config.poll_interval).await;

            let result_resp: serde_json::Value = self.client
                .post(format!("{}/getTaskResult", base))
                .json(&serde_json::json!({
                    "clientKey": api_key,
                    "taskId": task_id.parse::<i64>().unwrap_or(0),
                }))
                .send()
                .await?
                .json()
                .await?;

            if result_resp["errorId"].as_i64() != Some(0) {
                anyhow::bail!("anti-captcha poll error: {}", result_resp["errorDescription"]);
            }

            if result_resp["status"].as_str() == Some("ready") {
                // solution zależy od typu
                let solution = match captcha_type {
                    "image" => result_resp["solution"]["text"]
                        .as_str()
                        .context("missing text solution")?,
                    _ => result_resp["solution"]["gRecaptchaResponse"]
                        .as_str()
                        .or_else(|| result_resp["solution"]["text"].as_str())
                        .context("missing token solution")?,
                }.to_string();

                info!("anti-captcha: solved! type={}", captcha_type);
                return Ok(CaptchaSolution {
                    solution,
                    captcha_type: captcha_type.to_string(),
                    task_id,
                });
            }
            // status == "processing" - czekamy
        }
    }

    /// Wyciąga site_key reCAPTCHA v2 z HTML strony (szybki helper)
    pub fn extract_recaptcha_key(html: &str) -> Option<String> {
        // data-sitekey="6Lc..."
        let marker = "data-sitekey=\"";
        let start = html.find(marker)? + marker.len();
        let end = html[start..].find('"')?;
        Some(html[start..start + end].to_string())
    }

    /// Wyciąga hCaptcha site_key z HTML
    pub fn extract_hcaptcha_key(html: &str) -> Option<String> {
        // data-sitekey na elemencie z class h-captcha
        // szukamy bloku z h-captcha
        let marker = "h-captcha";
        let pos = html.find(marker)?;
        let snippet = &html[pos..pos.min(html.len()).min(pos + 300)];
        let key_marker = "data-sitekey=\"";
        let start = snippet.find(key_marker)? + key_marker.len();
        let end = snippet[start..].find('"')?;
        Some(snippet[start..start + end].to_string())
    }
}
