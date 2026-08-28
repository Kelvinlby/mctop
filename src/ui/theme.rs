//! Colours and the rules for choosing between them.
//!
//! The background is left at the terminal's own, so mctop sits inside whatever
//! colour scheme the operator already runs. Only the foreground is set.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::config::UiConfig;

/// How healthy a reading is. The whole interface reduces to these three, so a
/// glance at the colour is enough to know whether to look closer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Good,
    Warn,
    Bad,
    Unknown,
}

pub struct Theme {
    pub text: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    pub info: Color,
    pub highlight: Color,
    rounded: bool,
    tps_good: f64,
    tps_warn: f64,
    mspt_good: f64,
    mspt_warn: f64,
}

impl Theme {
    pub fn new(config: &UiConfig) -> Self {
        Self {
            text: Color::Rgb(216, 222, 233),
            dim: Color::Rgb(116, 125, 140),
            border: Color::Rgb(62, 70, 84),
            accent: Color::Rgb(129, 161, 193),
            good: Color::Rgb(122, 199, 137),
            warn: Color::Rgb(229, 192, 123),
            bad: Color::Rgb(224, 108, 117),
            info: Color::Rgb(102, 187, 203),
            highlight: Color::Rgb(45, 55, 70),
            rounded: config.rounded_borders,
            tps_good: config.tps_good,
            tps_warn: config.tps_warn,
            mspt_good: config.mspt_good,
            mspt_warn: config.mspt_warn,
        }
    }

    pub fn colour(&self, health: Health) -> Color {
        match health {
            Health::Good => self.good,
            Health::Warn => self.warn,
            Health::Bad => self.bad,
            Health::Unknown => self.dim,
        }
    }

    pub fn style(&self, health: Health) -> Style {
        Style::default().fg(self.colour(health))
    }

    /// Tick rate: healthy at the configured ceiling, critical below the floor.
    pub fn tps_health(&self, tps: Option<f64>) -> Health {
        match tps {
            None => Health::Unknown,
            Some(tps) if tps >= self.tps_good => Health::Good,
            Some(tps) if tps >= self.tps_warn => Health::Warn,
            Some(_) => Health::Bad,
        }
    }

    /// Tick duration, where lower is better. A tick has 50ms before the server
    /// starts falling behind, so the defaults sit well inside that.
    pub fn mspt_health(&self, mspt: Option<f64>) -> Health {
        match mspt {
            None => Health::Unknown,
            Some(mspt) if mspt <= self.mspt_good => Health::Good,
            Some(mspt) if mspt <= self.mspt_warn => Health::Warn,
            Some(_) => Health::Bad,
        }
    }

    /// A generic fraction of some capacity: heap occupancy, disk, CPU.
    pub fn load_health(&self, fraction: Option<f64>) -> Health {
        match fraction {
            None => Health::Unknown,
            Some(value) if value < 0.75 => Health::Good,
            Some(value) if value < 0.90 => Health::Warn,
            Some(_) => Health::Bad,
        }
    }

    /// A framed panel with a titled top edge.
    pub fn panel(&self, title: &str) -> Block<'static> {
        Block::bordered()
            .border_type(if self.rounded {
                BorderType::Rounded
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(self.border))
            .padding(Padding::horizontal(1))
            .title_top(ratatui::text::Line::from(vec![
                ratatui::text::Span::styled(
                    format!(" {title} "),
                    Style::default()
                        .fg(self.accent)
                        .add_modifier(Modifier::BOLD),
                ),
            ]))
    }

    pub fn label(&self) -> Style {
        Style::default().fg(self.dim)
    }

    pub fn value(&self) -> Style {
        Style::default().fg(self.text)
    }

    pub fn strong(&self) -> Style {
        Style::default().fg(self.text).add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::new(&UiConfig::default())
    }

    #[test]
    fn tps_health_follows_the_configured_thresholds() {
        let theme = theme();
        assert_eq!(theme.tps_health(Some(20.0)), Health::Good);
        assert_eq!(theme.tps_health(Some(19.5)), Health::Good);
        assert_eq!(theme.tps_health(Some(19.0)), Health::Warn);
        assert_eq!(theme.tps_health(Some(12.0)), Health::Bad);
        assert_eq!(theme.tps_health(None), Health::Unknown);
    }

    #[test]
    fn mspt_health_is_inverted() {
        let theme = theme();
        assert_eq!(theme.mspt_health(Some(3.0)), Health::Good);
        assert_eq!(theme.mspt_health(Some(30.0)), Health::Warn);
        assert_eq!(theme.mspt_health(Some(60.0)), Health::Bad);
    }

    #[test]
    fn thresholds_come_from_the_config() {
        let theme = Theme::new(&UiConfig {
            tps_good: 20.0,
            tps_warn: 19.9,
            ..UiConfig::default()
        });
        assert_eq!(theme.tps_health(Some(19.95)), Health::Warn);
    }

    #[test]
    fn load_health_climbs_with_the_fraction() {
        let theme = theme();
        assert_eq!(theme.load_health(Some(0.5)), Health::Good);
        assert_eq!(theme.load_health(Some(0.8)), Health::Warn);
        assert_eq!(theme.load_health(Some(0.95)), Health::Bad);
    }
}
