use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRate {
    Unlimited,
    Fps30,
    Fps60,
    Fps120,
}

impl FrameRate {
    pub fn cycle(self) -> Self {
        match self {
            Self::Unlimited => Self::Fps120,
            Self::Fps120 => Self::Fps60,
            Self::Fps60 => Self::Fps30,
            Self::Fps30 => Self::Unlimited,
        }
    }

    pub fn interval(self) -> Option<Duration> {
        match self {
            Self::Unlimited => None,
            Self::Fps30 => Some(Duration::from_nanos(33_333_333)),
            Self::Fps60 => Some(Duration::from_nanos(16_666_667)),
            Self::Fps120 => Some(Duration::from_nanos(8_333_333)),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Unlimited => "Unlimited",
            Self::Fps30 => "30fps",
            Self::Fps60 => "60fps",
            Self::Fps120 => "120fps",
        }
    }
}
