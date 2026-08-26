//! Activity statistics for the profile screen.
//!
//! Deliberately pure: it takes facts that were gathered elsewhere and derives
//! numbers from them, so streak arithmetic and heatmap bucketing can be tested
//! without a workspace, a ledger file, or a clock.
//!
//! Two sources with different reach, and the difference is visible in the output
//! rather than smoothed over:
//!
//! - **Chats** come from thread files, so they are retroactive — every
//!   conversation that already exists counts.
//! - **Tokens** come from the ledger's daily buckets, which only started being
//!   written when that feature landed. Days before then have real chat counts
//!   and no token figure, which is why [`DayPoint`] carries both and the UI can
//!   say which it is drawing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::usage::{day_number_from_key, local_day_number, DayUsage};

/// What the profile needs to know about one conversation.
#[derive(Debug, Clone)]
pub struct ChatFacts {
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
}

/// One cell of the heatmap.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DayPoint {
    pub date: String,
    pub chats: u32,
    pub messages: u32,
    /// `None` for a day that predates token metering — distinct from a day that
    /// was metered and genuinely spent nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
}

impl DayPoint {
    fn is_active(&self) -> bool {
        self.chats > 0 || self.requests.is_some_and(|r| r > 0)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStats {
    pub total_chats: u32,
    pub total_messages: u32,
    /// Lifetime tokens from the per-provider totals, which predate daily
    /// buckets — so this is larger than the sum of `days` on an existing install
    /// rather than disagreeing with it.
    pub total_tokens: u64,
    pub total_requests: u64,
    /// Busiest metered day. Zero until there is a day of metering.
    pub peak_day_tokens: u64,
    /// Longest single conversation, wall clock from first to last message.
    pub longest_chat_secs: u64,
    pub current_streak_days: u32,
    pub longest_streak_days: u32,
    /// Unix seconds of the earliest conversation, if there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_activity: Option<u64>,
    /// Every day that has chats or metered requests, oldest first.
    pub days: Vec<DayPoint>,
    /// The day metering began, so the UI can shade earlier cells as "no data"
    /// instead of implying zero spend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metering_since: Option<String>,
}

/// Derive the profile from gathered chats and the ledger's daily buckets.
///
/// `today` is passed in rather than read from the clock so the streak boundary
/// is testable.
pub fn derive(
    chats: &[ChatFacts],
    daily: &BTreeMap<String, DayUsage>,
    lifetime_tokens: u64,
    lifetime_requests: u64,
    today: i64,
) -> ProfileStats {
    let mut by_day: BTreeMap<i64, DayPoint> = BTreeMap::new();

    for chat in chats {
        let day = local_day_number(chat.created_at);
        let point = by_day.entry(day).or_default();
        point.chats += 1;
        point.messages += chat.message_count as u32;
    }

    for (key, usage) in daily {
        let Some(day) = day_number_from_key(key) else {
            continue;
        };
        let point = by_day.entry(day).or_default();
        point.tokens = Some(usage.total_tokens());
        point.requests = Some(usage.requests);
    }

    for (day, point) in by_day.iter_mut() {
        point.date = crate::usage::day_key_from_number(*day);
    }

    let active: Vec<i64> = by_day
        .iter()
        .filter(|(_, p)| p.is_active())
        .map(|(d, _)| *d)
        .collect();

    ProfileStats {
        total_chats: chats.len() as u32,
        total_messages: chats.iter().map(|c| c.message_count as u32).sum(),
        total_tokens: lifetime_tokens,
        total_requests: lifetime_requests,
        peak_day_tokens: daily
            .values()
            .map(DayUsage::total_tokens)
            .max()
            .unwrap_or(0),
        longest_chat_secs: chats
            .iter()
            .map(|c| c.updated_at.saturating_sub(c.created_at))
            .max()
            .unwrap_or(0),
        current_streak_days: current_streak(&active, today),
        longest_streak_days: longest_streak(&active),
        first_activity: chats.iter().map(|c| c.created_at).min(),
        days: by_day.into_values().collect(),
        metering_since: daily.keys().next().cloned(),
    }
}

/// Consecutive active days ending today or yesterday.
///
/// Yesterday still counts: a streak should not appear broken for the whole of
/// today just because the day's work has not started yet.
fn current_streak(active_days: &[i64], today: i64) -> u32 {
    let Some(&last) = active_days.last() else {
        return 0;
    };
    if last < today - 1 {
        return 0;
    }
    let mut streak = 0;
    let mut expected = last;
    for &day in active_days.iter().rev() {
        if day == expected {
            streak += 1;
            expected -= 1;
        } else if day < expected {
            break;
        }
    }
    streak
}

fn longest_streak(active_days: &[i64]) -> u32 {
    let mut best = 0;
    let mut run = 0;
    let mut previous: Option<i64> = None;
    for &day in active_days {
        run = match previous {
            Some(p) if day == p + 1 => run + 1,
            _ => 1,
        };
        best = best.max(run);
        previous = Some(day);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(created: u64, updated: u64, messages: usize) -> ChatFacts {
        ChatFacts {
            created_at: created,
            updated_at: updated,
            message_count: messages,
        }
    }

    /// Day N at noon UTC, so a test never sits on a midnight boundary.
    fn at(day: i64) -> u64 {
        (day * 86_400 + 43_200) as u64
    }

    #[test]
    fn a_streak_runs_up_to_today() {
        let chats = vec![
            chat(at(10), at(10), 2),
            chat(at(11), at(11), 2),
            chat(at(12), at(12), 2),
        ];
        let stats = derive(&chats, &BTreeMap::new(), 0, 0, 12);
        assert_eq!(stats.current_streak_days, 3);
        assert_eq!(stats.longest_streak_days, 3);
    }

    /// Yesterday still counts. Today's work may simply not have started.
    #[test]
    fn a_streak_survives_until_a_full_day_is_missed() {
        let chats = vec![chat(at(9), at(9), 1), chat(at(10), at(10), 1)];
        assert_eq!(
            derive(&chats, &BTreeMap::new(), 0, 0, 11).current_streak_days,
            2
        );
        assert_eq!(
            derive(&chats, &BTreeMap::new(), 0, 0, 12).current_streak_days,
            0
        );
    }

    #[test]
    fn the_longest_streak_survives_a_later_gap() {
        // Five in a row long ago, two recent. The record stands.
        let mut chats: Vec<_> = (1..=5).map(|d| chat(at(d), at(d), 1)).collect();
        chats.push(chat(at(20), at(20), 1));
        chats.push(chat(at(21), at(21), 1));
        let stats = derive(&chats, &BTreeMap::new(), 0, 0, 21);
        assert_eq!(stats.longest_streak_days, 5);
        assert_eq!(stats.current_streak_days, 2);
    }

    #[test]
    fn several_chats_in_one_day_are_one_day_of_streak() {
        let chats = vec![chat(at(5), at(5), 3), chat(at(5) + 3600, at(5) + 7200, 4)];
        let stats = derive(&chats, &BTreeMap::new(), 0, 0, 5);
        assert_eq!(stats.current_streak_days, 1);
        assert_eq!(stats.total_chats, 2);
        assert_eq!(stats.total_messages, 7);
        assert_eq!(stats.days.len(), 1, "one heatmap cell");
        assert_eq!(stats.days[0].chats, 2);
    }

    #[test]
    fn no_activity_is_no_streak() {
        let stats = derive(&[], &BTreeMap::new(), 0, 0, 100);
        assert_eq!(stats.current_streak_days, 0);
        assert_eq!(stats.longest_streak_days, 0);
        assert_eq!(stats.total_chats, 0);
        assert!(stats.days.is_empty());
        assert_eq!(stats.first_activity, None);
    }

    #[test]
    fn the_longest_chat_is_measured_end_to_end() {
        let chats = vec![
            chat(at(1), at(1) + 600, 4),
            chat(at(2), at(2) + 4_380, 20),
            chat(at(3), at(3) + 60, 2),
        ];
        let stats = derive(&chats, &BTreeMap::new(), 0, 0, 3);
        assert_eq!(stats.longest_chat_secs, 4_380);
    }

    /// The point of keeping `tokens` optional: a day with chats but no metering
    /// must not claim zero spend.
    #[test]
    fn days_before_metering_report_no_token_figure() {
        let chats = vec![chat(at(1), at(1), 2), chat(at(9), at(9), 2)];
        let mut daily = BTreeMap::new();
        daily.insert(
            crate::usage::day_key_from_number(9),
            DayUsage {
                requests: 3,
                input_tokens: 100,
                output_tokens: 20,
                ..Default::default()
            },
        );

        let stats = derive(&chats, &daily, 5_000, 42, 9);
        let old = stats
            .days
            .iter()
            .find(|d| d.chats > 0 && d.tokens.is_none());
        assert!(
            old.is_some(),
            "the pre-metering day keeps a null token figure"
        );

        let metered = stats.days.last().unwrap();
        assert_eq!(metered.tokens, Some(120));
        assert_eq!(metered.requests, Some(3));
        assert_eq!(stats.peak_day_tokens, 120);
        assert_eq!(stats.metering_since.as_deref(), Some(metered.date.as_str()));
        // Lifetime totals come from the per-provider ledger, which is older than
        // the daily buckets, so they are legitimately larger.
        assert_eq!(stats.total_tokens, 5_000);
        assert_eq!(stats.total_requests, 42);
    }

    /// A day with metered requests but no new chat still counts as active: work
    /// continued in a conversation that started earlier.
    #[test]
    fn a_metered_day_with_no_new_chat_keeps_the_streak_alive() {
        let chats = vec![chat(at(1), at(3), 2)];
        let mut daily = BTreeMap::new();
        for day in 2..=3 {
            daily.insert(
                crate::usage::day_key_from_number(day),
                DayUsage {
                    requests: 1,
                    output_tokens: 10,
                    ..Default::default()
                },
            );
        }
        let stats = derive(&chats, &daily, 30, 3, 3);
        assert_eq!(stats.current_streak_days, 3, "days 1, 2 and 3");
    }

    #[test]
    fn heatmap_cells_are_dated_and_ordered() {
        let chats = vec![chat(at(20), at(20), 1), chat(at(2), at(2), 1)];
        let stats = derive(&chats, &BTreeMap::new(), 0, 0, 20);
        let dates: Vec<_> = stats.days.iter().map(|d| d.date.as_str()).collect();
        assert_eq!(dates, vec!["1970-01-03", "1970-01-21"], "oldest first");
    }
}
