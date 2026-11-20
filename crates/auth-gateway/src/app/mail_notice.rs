//! How emails look: a [`MailNotice`] is built from blocks (paragraphs, facts,
//! a section, a button…) and both parts of the email come from it, plain text
//! and HTML, with the same content. The HTML is tables and inline styles, the
//! only thing Outlook and Gmail respect.

use super::mail::OutgoingMail;

/// How serious it is: the color of the stripe and the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailTone {
    /// News or a step to take.
    Info,
    /// Something that went well or works again.
    Success,
    /// Something to look at soon.
    Warning,
    /// Something that failed or is urgent.
    Danger,
}

impl MailTone {
    /// The stripe's color and the label's, with its background.
    fn colors(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Info => ("#4C6EF5", "#364FC7", "#EDF2FF"),
            Self::Success => ("#2F9E44", "#2B8A3E", "#EBFBEE"),
            Self::Warning => ("#F5A524", "#A86B00", "#FFF6E0"),
            Self::Danger => ("#E03131", "#C92A2A", "#FFF5F5"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    /// Prose: paragraphs are separated by a blank line.
    Prose(String),
    /// Pares etiqueta–valor.
    Facts(Vec<(String, String)>),
    /// The title of a part of the email.
    Heading(String),
    /// Verbatim text (an error, a log), with its label.
    Code(String, String),
    /// A list.
    List(Vec<String>),
    /// A box for what must not be overlooked.
    Callout(String),
    /// A button with its link (and the link in view, in case the button does not show).
    Action(String, String),
}

/// A formatted email. It is built with its methods, in reading order, and
/// [`Self::mail`] gives the [`OutgoingMail`] with the text and the HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailNotice {
    tone: MailTone,
    badge: Option<String>,
    title: String,
    blocks: Vec<Block>,
    footer: Vec<String>,
}

impl MailNotice {
    pub fn new(tone: MailTone, title: impl Into<String>) -> Self {
        Self {
            tone,
            badge: None,
            title: title.into(),
            blocks: Vec::new(),
            footer: Vec::new(),
        }
    }

    /// The label above the title ("Failed execution", "Security").
    #[must_use]
    pub fn badge(mut self, text: impl Into<String>) -> Self {
        self.badge = Some(text.into());
        self
    }

    /// One or more paragraphs (separated by a blank line). Empty adds nothing.
    #[must_use]
    pub fn prose(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        if !text.trim().is_empty() {
            self.blocks.push(Block::Prose(text.trim().to_string()));
        }
        self
    }

    /// Label–value pairs; those with an empty value are not shown.
    #[must_use]
    pub fn facts<L: Into<String>, V: Into<String>>(
        mut self,
        facts: impl IntoIterator<Item = (L, V)>,
    ) -> Self {
        let facts: Vec<(String, String)> = facts
            .into_iter()
            .map(|(l, v)| (l.into(), v.into()))
            .filter(|(_, v)| !v.trim().is_empty())
            .collect();
        if !facts.is_empty() {
            self.blocks.push(Block::Facts(facts));
        }
        self
    }

    #[must_use]
    pub fn heading(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Heading(text.into()));
        self
    }

    /// Verbatim text, like an error or a command's output.
    #[must_use]
    pub fn code(mut self, label: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        if !text.trim().is_empty() {
            self.blocks
                .push(Block::Code(label.into(), text.trim().to_string()));
        }
        self
    }

    #[must_use]
    pub fn list<T: Into<String>>(mut self, items: impl IntoIterator<Item = T>) -> Self {
        let items: Vec<String> = items.into_iter().map(Into::into).collect();
        if !items.is_empty() {
            self.blocks.push(Block::List(items));
        }
        self
    }

    #[must_use]
    pub fn callout(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        if !text.trim().is_empty() {
            self.blocks.push(Block::Callout(text.trim().to_string()));
        }
        self
    }

    #[must_use]
    pub fn action(mut self, label: impl Into<String>, url: impl Into<String>) -> Self {
        self.blocks.push(Block::Action(label.into(), url.into()));
        self
    }

    /// Why the email reaches them, in small print at the foot.
    #[must_use]
    pub fn footer(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        if !text.trim().is_empty() {
            self.footer.push(text.trim().to_string());
        }
        self
    }

    /// The email for `to` with its subject.
    pub fn mail(&self, to: Vec<String>, subject: impl Into<String>) -> OutgoingMail {
        let subject = subject.into();
        OutgoingMail {
            to,
            html: Some(self.html(&subject)),
            subject,
            text: self.text(),
        }
    }

    /// The plain text part.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::Prose(text) | Block::Callout(text) => {
                    out += text;
                    out += "\n\n";
                }
                Block::Facts(facts) => {
                    for (label, value) in facts {
                        out += &format!("{label}: {value}\n");
                    }
                    out += "\n";
                }
                Block::Heading(text) => out += &format!("— {text}\n"),
                Block::Code(label, text) => out += &format!("{label}:\n{text}\n\n"),
                Block::List(items) => {
                    for item in items {
                        out += &format!("- {item}\n");
                    }
                    out += "\n";
                }
                Block::Action(label, url) => out += &format!("{label}:\n{url}\n\n"),
            }
        }
        for line in &self.footer {
            out += line;
            out += "\n\n";
        }
        out += &texts::tr!("app.mail.footer");
        out += "\n";
        out
    }

    /// The HTML part.
    pub fn html(&self, subject: &str) -> String {
        let (stripe, badge_fg, badge_bg) = self.tone.colors();
        let lang = texts::current().code();
        let preheader = self
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Prose(t) | Block::Callout(t) => {
                    Some(t.split("\n\n").next().unwrap_or_default().to_string())
                }
                _ => None,
            })
            .unwrap_or_default();

        let mut body = String::new();
        if let Some(badge) = &self.badge {
            body += &format!(
                r#"<p style="margin:0 0 12px"><span style="display:inline-block;padding:3px 10px;border-radius:999px;background:{badge_bg};color:{badge_fg};font-size:12px;font-weight:600;letter-spacing:.02em">{}</span></p>"#,
                esc(badge)
            );
        }
        body += &format!(
            r#"<h1 style="margin:0 0 20px;font-size:22px;line-height:1.3;font-weight:700;color:#1A1B1E">{}</h1>"#,
            esc(&self.title)
        );
        for block in &self.blocks {
            body += &match block {
                Block::Prose(text) => text
                    .split("\n\n")
                    .map(|p| format!(r#"<p style="margin:0 0 16px">{}</p>"#, lines(p)))
                    .collect(),
                Block::Facts(facts) => {
                    let rows: String = facts
                        .iter()
                        .map(|(label, value)| {
                            format!(
                                r#"<tr><td style="padding:8px 12px;border-bottom:1px solid #E9ECEF;color:#5C5F66;font-size:13px;white-space:nowrap;vertical-align:top;width:1%">{}</td><td style="padding:8px 12px;border-bottom:1px solid #E9ECEF;color:#1A1B1E;font-size:14px;font-weight:500">{}</td></tr>"#,
                                esc(label),
                                lines(value)
                            )
                        })
                        .collect();
                    format!(
                        r#"<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="margin:0 0 20px;border:1px solid #E9ECEF;border-radius:8px;border-collapse:separate;background:#F8F9FA">{rows}</table>"#
                    )
                }
                Block::Heading(text) => format!(
                    r#"<h2 style="margin:24px 0 8px;font-size:16px;line-height:1.4;font-weight:700;color:#1A1B1E">{}</h2>"#,
                    esc(text)
                ),
                Block::Code(label, text) => format!(
                    r#"<p style="margin:0 0 6px;font-size:13px;font-weight:600;color:#5C5F66">{}</p><pre style="margin:0 0 20px;padding:12px 14px;background:#F1F3F5;border-radius:6px;font-family:Consolas,Menlo,monospace;font-size:12px;line-height:1.5;color:#1A1B1E;white-space:pre-wrap;word-break:break-word">{}</pre>"#,
                    esc(label),
                    esc(text)
                ),
                Block::List(items) => {
                    let items: String = items
                        .iter()
                        .map(|i| format!(r#"<li style="margin:0 0 4px">{}</li>"#, esc(i)))
                        .collect();
                    format!(r#"<ul style="margin:0 0 16px;padding-left:20px">{items}</ul>"#)
                }
                Block::Callout(text) => format!(
                    r#"<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="margin:0 0 20px"><tr><td style="padding:12px 16px;background:{badge_bg};border-left:4px solid {stripe};border-radius:4px;color:#1A1B1E;font-size:14px">{}</td></tr></table>"#,
                    paragraphs(text)
                ),
                Block::Action(label, url) => format!(
                    r#"<table role="presentation" cellpadding="0" cellspacing="0" style="margin:8px 0 12px"><tr><td style="border-radius:6px;background:#4C6EF5"><a href="{href}" style="display:inline-block;padding:12px 22px;color:#FFFFFF;font-size:15px;font-weight:600;text-decoration:none;border-radius:6px">{label}</a></td></tr></table><p style="margin:0 0 20px;font-size:12px;color:#868E96">{fallback}<br><a href="{href}" style="color:#4C6EF5;word-break:break-all">{shown}</a></p>"#,
                    href = esc(url),
                    label = esc(label),
                    shown = esc(url),
                    fallback = esc(&texts::tr!("app.mail.buttonFallback")),
                ),
            };
        }
        let footer: String = self
            .footer
            .iter()
            .map(|f| format!(r#"<p style="margin:0 0 8px">{}</p>"#, lines(f)))
            .collect();

        format!(
            r#"<!DOCTYPE html>
<html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light"><meta name="supported-color-schemes" content="light"><title>{subject}</title></head>
<body style="margin:0;padding:0;background:#F1F3F5;-webkit-text-size-adjust:100%">
<div style="display:none;max-height:0;overflow:hidden;opacity:0;color:transparent">{preheader}</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background:#F1F3F5"><tr><td align="center" style="padding:32px 16px">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="max-width:600px">
<tr><td style="padding:0 4px 16px"><table role="presentation" cellpadding="0" cellspacing="0"><tr><td style="width:22px;height:22px;background:#F5A524;border:1.5px solid #C98612;border-radius:6px;font-size:0;line-height:0">&nbsp;</td><td style="padding-left:10px;font-family:'Segoe UI',Roboto,Helvetica,Arial,sans-serif;font-size:15px;font-weight:700;color:#1A1B1E">{brand}</td></tr></table></td></tr>
<tr><td style="background:#FFFFFF;border-radius:10px;border-top:4px solid {stripe};padding:32px;font-family:'Segoe UI',Roboto,Helvetica,Arial,sans-serif;font-size:15px;line-height:1.6;color:#373A40">
{body}</td></tr>
<tr><td style="padding:20px 8px 0;font-family:'Segoe UI',Roboto,Helvetica,Arial,sans-serif;font-size:12px;line-height:1.5;color:#868E96">{footer}<p style="margin:0">{pie}</p></td></tr>
</table></td></tr></table>
</body></html>
"#,
            subject = esc(subject),
            preheader = esc(&preheader),
            brand = esc(&texts::tr!("app.mail.brand")),
            pie = esc(&texts::tr!("app.mail.footer")),
        )
    }
}

/// Escapes for HTML.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out += "&amp;",
            '<' => out += "&lt;",
            '>' => out += "&gt;",
            '"' => out += "&quot;",
            '\'' => out += "&#39;",
            c => out.push(c),
        }
    }
    out
}

/// Escapes and turns line breaks into `<br>`.
fn lines(text: &str) -> String {
    esc(text.trim()).replace('\n', "<br>")
}

/// Several paragraphs in one box.
fn paragraphs(text: &str) -> String {
    text.split("\n\n")
        .map(lines)
        .collect::<Vec<_>>()
        .join("<br><br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice() -> MailNotice {
        MailNotice::new(MailTone::Danger, "«Cierre» falló")
            .badge("Ejecución")
            .prose("Primero.\n\nSegundo <b>.")
            .facts([("Área", "Finanzas"), ("Vacía", "")])
            .code("Error", "boom & <x>")
            .action("Abrir", "https://x.co/a?b=1&c=2")
            .footer("Por qué te llega.")
    }

    #[test]
    fn the_text_part_has_every_block_in_order() {
        let text = notice().text();
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s:?} en {text}"));
        assert!(at("Primero.") < at("Segundo <b>."));
        assert!(at("Área: Finanzas") < at("Error:\nboom & <x>"));
        assert!(at("Abrir:\nhttps://x.co/a?b=1&c=2") < at("Por qué te llega."));
        assert!(!text.contains("Vacía"), "{text}");
    }

    #[test]
    fn the_html_part_escapes_what_it_shows() {
        let html = notice().html("asunto <y>");
        assert!(html.contains("Segundo &lt;b&gt;."));
        assert!(html.contains("boom &amp; &lt;x&gt;"));
        assert!(html.contains(r#"href="https://x.co/a?b=1&amp;c=2""#));
        assert!(html.contains("<title>asunto &lt;y&gt;</title>"));
        assert!(html.contains("«Cierre» falló"));
        assert!(html.contains("#E03131"), "la franja del tono");
        assert!(!html.contains("Vacía"));
    }

    #[test]
    fn the_mail_carries_both_parts() {
        let mail = notice().mail(vec!["a@b.co".into()], "Asunto");
        assert_eq!(mail.subject, "Asunto");
        assert!(mail.text.contains("Primero."));
        assert!(mail.html.as_deref().unwrap().starts_with("<!DOCTYPE html>"));
    }
}
