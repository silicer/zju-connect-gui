use rand::Rng;
use std::time::Duration;

pub const DEFAULT_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
pub const DEFAULT_RETRY_MAX_DELAY: Duration = Duration::from_secs(60);

const JITTER_FRACTION: f64 = 0.2;
const RETRY_FLOOR: Duration = Duration::from_secs(1);

pub type JitterFn = fn(Duration, u32) -> Duration;

pub fn next_retry_delay(attempt: u32, base: Duration, max: Duration, jitter: JitterFn) -> Duration {
    let mut delay = if base.is_zero() { RETRY_FLOOR } else { base };
    let cap = if max.is_zero() {
        DEFAULT_RETRY_MAX_DELAY
    } else {
        max
    };
    let mut i = 1u32;
    while i < attempt {
        if delay >= cap {
            break;
        }
        delay = delay.saturating_mul(2);
        if delay > cap {
            delay = cap;
        }
        i += 1;
    }
    delay = jitter(delay, attempt);
    if delay < RETRY_FLOOR {
        delay = RETRY_FLOOR;
    }
    if delay > cap {
        delay = cap;
    }
    delay
}

pub fn default_jitter(delay: Duration, _attempt: u32) -> Duration {
    let nanos = delay.as_nanos() as i128;
    if nanos <= 0 {
        return RETRY_FLOOR;
    }
    let spread = (nanos as f64 * JITTER_FRACTION) as i128;
    if spread <= 0 {
        return delay;
    }
    let mut rng = rand::thread_rng();
    let offset = rng.gen_range(-spread..=spread);
    let jittered = nanos + offset;
    if jittered <= 0 {
        return RETRY_FLOOR;
    }
    Duration::from_nanos(jittered as u64)
}

pub fn no_jitter(delay: Duration, _attempt: u32) -> Duration {
    delay
}

pub fn format_retry_delay(delay: Duration) -> String {
    let total_seconds = (delay.as_secs_f64()).round() as u64;
    if total_seconds == 0 {
        return format!("{:?}", delay);
    }
    if total_seconds < 60 {
        return format!("{total_seconds} 秒");
    }
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if seconds == 0 {
        format!("{minutes} 分钟")
    } else {
        format!("{minutes} 分 {seconds} 秒")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_retry_delay_no_jitter_doubles() {
        let base = Duration::from_secs(1);
        let max = Duration::from_secs(60);
        assert_eq!(
            next_retry_delay(1, base, max, no_jitter),
            Duration::from_secs(1)
        );
        assert_eq!(
            next_retry_delay(2, base, max, no_jitter),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_retry_delay(3, base, max, no_jitter),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_retry_delay(4, base, max, no_jitter),
            Duration::from_secs(8)
        );
        assert_eq!(
            next_retry_delay(5, base, max, no_jitter),
            Duration::from_secs(16)
        );
        assert_eq!(
            next_retry_delay(6, base, max, no_jitter),
            Duration::from_secs(32)
        );
        assert_eq!(
            next_retry_delay(7, base, max, no_jitter),
            Duration::from_secs(60)
        );
        assert_eq!(
            next_retry_delay(8, base, max, no_jitter),
            Duration::from_secs(60)
        );
        assert_eq!(
            next_retry_delay(20, base, max, no_jitter),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn default_jitter_within_twenty_percent() {
        let base = Duration::from_secs(10);
        for _ in 0..50 {
            let jittered = default_jitter(base, 1);
            assert!(
                jittered >= Duration::from_secs(8) && jittered <= Duration::from_secs(12),
                "{jittered:?} outside ±20% window"
            );
        }
    }

    #[test]
    fn next_retry_delay_floors_at_one_second() {
        let small = Duration::from_millis(50);
        let max = Duration::from_secs(10);
        let result = next_retry_delay(1, small, max, |_, _| Duration::from_millis(10));
        assert_eq!(result, RETRY_FLOOR);
    }

    #[test]
    fn format_retry_delay_humanized() {
        assert_eq!(format_retry_delay(Duration::from_secs(1)), "1 秒");
        assert_eq!(format_retry_delay(Duration::from_secs(45)), "45 秒");
        assert_eq!(format_retry_delay(Duration::from_secs(60)), "1 分钟");
        assert_eq!(format_retry_delay(Duration::from_secs(125)), "2 分 5 秒");
    }
}
