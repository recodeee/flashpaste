use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

pub const DEFAULT_TTL_MS: u32 = 3_000;
pub const MAX_TTL_MS: u32 = 30_000;
pub const DEFAULT_STROKE_WIDTH: f64 = 2.0;
pub const DEFAULT_CURRENT_OPACITY: f64 = 1.0;
pub const MAX_LABEL_CHARS: usize = 200;
pub const MAX_COORD_ABS: f64 = 1_000_000.0;
pub const MAX_STROKE_WIDTH: f64 = 1_000.0;

pub type RectShape = DrawRect;
pub type CircleShape = DrawCircle;
pub type ArrowShape = DrawArrow;
pub type LabelShape = DrawLabel;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    DrawRect(DrawRect),
    DrawCircle(DrawCircle),
    DrawArrow(DrawArrow),
    DrawLabel(DrawLabel),
    Clear(Clear),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawStyle {
    pub id: Uuid,
    #[serde(default = "default_ttl_ms", deserialize_with = "deserialize_ttl_ms")]
    pub ttl_ms: u32,
    #[serde(default)]
    pub color: Color,
    #[serde(
        default = "default_stroke_width",
        deserialize_with = "deserialize_stroke_width"
    )]
    pub stroke_width: f64,
    #[serde(default = "default_current_opacity", skip)]
    pub current_opacity: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawRect {
    #[serde(flatten)]
    pub style: DrawStyle,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub x: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub y: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub w: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub h: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawCircle {
    #[serde(flatten)]
    pub style: DrawStyle,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub x: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub y: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub w: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub h: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawArrow {
    #[serde(flatten)]
    pub style: DrawStyle,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub x1: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub y1: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub x2: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub y2: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawLabel {
    #[serde(flatten)]
    pub style: DrawStyle,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub x: f64,
    #[serde(deserialize_with = "deserialize_coordinate")]
    pub y: f64,
    #[serde(deserialize_with = "deserialize_label_text")]
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Clear {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Response {
    Error(ErrorResponse),
    Ok(OkResponse),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OkResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub ok: bool,
    pub error: String,
}

impl Response {
    pub fn ok(id: Uuid) -> Self {
        Self::Ok(OkResponse {
            ok: true,
            id: Some(id),
        })
    }

    pub fn ok_without_id() -> Self {
        Self::Ok(OkResponse { ok: true, id: None })
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self::Error(ErrorResponse {
            ok: false,
            error: error.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub f64, pub f64, pub f64, pub f64);

impl Color {
    pub fn to_rgba_tuple(self) -> (f64, f64, f64, f64) {
        (self.0, self.1, self.2, self.3)
    }

    fn as_hex(self) -> String {
        let r = channel_to_u8(self.0);
        let g = channel_to_u8(self.1);
        let b = channel_to_u8(self.2);
        let a = channel_to_u8(self.3);

        if a == u8::MAX {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self(1.0, 174.0 / 255.0, 0.0, 1.0)
    }
}

impl FromStr for Color {
    type Err = ColorParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let hex = input
            .strip_prefix('#')
            .ok_or(ColorParseError::MissingHash)?;

        match hex.len() {
            6 | 8 => {
                let r = parse_hex_byte(hex, 0)?;
                let g = parse_hex_byte(hex, 2)?;
                let b = parse_hex_byte(hex, 4)?;
                let a = if hex.len() == 8 {
                    parse_hex_byte(hex, 6)?
                } else {
                    u8::MAX
                };

                Ok(Self(
                    f64::from(r) / 255.0,
                    f64::from(g) / 255.0,
                    f64::from(b) / 255.0,
                    f64::from(a) / 255.0,
                ))
            }
            len => Err(ColorParseError::InvalidLength(len + 1)),
        }
    }
}

impl Serialize for Color {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;
        input.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorParseError {
    MissingHash,
    InvalidLength(usize),
    InvalidHex(String),
}

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHash => write!(f, "color must start with #"),
            Self::InvalidLength(len) => {
                write!(f, "color must be #rrggbb or #rrggbbaa, got length {len}")
            }
            Self::InvalidHex(value) => write!(f, "invalid hex color component {value:?}"),
        }
    }
}

impl std::error::Error for ColorParseError {}

pub fn default_ttl_ms() -> u32 {
    DEFAULT_TTL_MS
}

pub fn default_stroke_width() -> f64 {
    DEFAULT_STROKE_WIDTH
}

pub fn default_current_opacity() -> f64 {
    DEFAULT_CURRENT_OPACITY
}

fn deserialize_ttl_ms<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let ttl_ms = u32::deserialize(deserializer)?;
    if ttl_ms <= MAX_TTL_MS {
        Ok(ttl_ms)
    } else {
        Err(de::Error::custom(format!("ttl_ms must be <= {MAX_TTL_MS}")))
    }
}

fn deserialize_label_text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    if text.chars().count() > MAX_LABEL_CHARS {
        return Err(de::Error::custom(format!(
            "text must be <= {MAX_LABEL_CHARS} characters"
        )));
    }

    if text.chars().any(char::is_control) {
        return Err(de::Error::custom(
            "text must not contain control characters",
        ));
    }

    Ok(text)
}

fn deserialize_coordinate<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f64::deserialize(deserializer)?;
    if value.is_finite() && value.abs() <= MAX_COORD_ABS {
        Ok(value)
    } else {
        Err(de::Error::custom(format!(
            "coordinate must be finite and within +/-{MAX_COORD_ABS}"
        )))
    }
}

fn deserialize_stroke_width<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f64::deserialize(deserializer)?;
    if value.is_finite() && value > 0.0 && value <= MAX_STROKE_WIDTH {
        Ok(value)
    } else {
        Err(de::Error::custom(format!(
            "stroke_width must be finite and in 0.0..={MAX_STROKE_WIDTH}"
        )))
    }
}

fn parse_hex_byte(hex: &str, start: usize) -> Result<u8, ColorParseError> {
    let value = &hex[start..start + 2];
    u8::from_str_radix(value, 16).map_err(|_| ColorParseError::InvalidHex(value.to_string()))
}

fn channel_to_u8(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn id(seed: u128) -> Uuid {
        Uuid::from_u128(seed)
    }

    fn style(seed: u128) -> DrawStyle {
        DrawStyle {
            id: id(seed),
            ttl_ms: 5_000,
            color: "#11223344".parse().unwrap(),
            stroke_width: 3.5,
            current_opacity: DEFAULT_CURRENT_OPACITY,
        }
    }

    fn assert_round_trip(message: Message) {
        let encoded = serde_json::to_string(&message).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_draw_rect() {
        assert_round_trip(Message::DrawRect(DrawRect {
            style: style(1),
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        }));
    }

    #[test]
    fn round_trips_draw_circle() {
        assert_round_trip(Message::DrawCircle(DrawCircle {
            style: style(2),
            x: 11.0,
            y: 21.0,
            w: 31.0,
            h: 41.0,
        }));
    }

    #[test]
    fn round_trips_draw_arrow() {
        assert_round_trip(Message::DrawArrow(DrawArrow {
            style: style(3),
            x1: 12.0,
            y1: 22.0,
            x2: 32.0,
            y2: 42.0,
        }));
    }

    #[test]
    fn round_trips_draw_label() {
        assert_round_trip(Message::DrawLabel(DrawLabel {
            style: style(4),
            x: 13.0,
            y: 23.0,
            text: "look here".to_string(),
        }));
    }

    #[test]
    fn round_trips_clear_with_id() {
        assert_round_trip(Message::Clear(Clear { id: Some(id(5)) }));
    }

    #[test]
    fn round_trips_clear_all() {
        assert_round_trip(Message::Clear(Clear { id: None }));
    }

    #[test]
    fn defaults_apply_to_rect() {
        let value = json!({
            "type": "draw_rect",
            "id": id(10),
            "x": 1.0,
            "y": 2.0,
            "w": 3.0,
            "h": 4.0
        });

        let Message::DrawRect(rect) = serde_json::from_value(value).unwrap() else {
            panic!("expected draw_rect");
        };

        assert_default_style(rect.style);
    }

    #[test]
    fn defaults_apply_to_circle() {
        let value = json!({
            "type": "draw_circle",
            "id": id(11),
            "x": 1.0,
            "y": 2.0,
            "w": 3.0,
            "h": 4.0
        });

        let Message::DrawCircle(circle) = serde_json::from_value(value).unwrap() else {
            panic!("expected draw_circle");
        };

        assert_default_style(circle.style);
    }

    #[test]
    fn defaults_apply_to_arrow() {
        let value = json!({
            "type": "draw_arrow",
            "id": id(12),
            "x1": 1.0,
            "y1": 2.0,
            "x2": 3.0,
            "y2": 4.0
        });

        let Message::DrawArrow(arrow) = serde_json::from_value(value).unwrap() else {
            panic!("expected draw_arrow");
        };

        assert_default_style(arrow.style);
    }

    #[test]
    fn defaults_apply_to_label() {
        let value = json!({
            "type": "draw_label",
            "id": id(13),
            "x": 1.0,
            "y": 2.0,
            "text": "label"
        });

        let Message::DrawLabel(label) = serde_json::from_value(value).unwrap() else {
            panic!("expected draw_label");
        };

        assert_default_style(label.style);
    }

    #[test]
    fn default_clear_id_is_none() {
        let Message::Clear(clear) = serde_json::from_value(json!({ "type": "clear" })).unwrap()
        else {
            panic!("expected clear");
        };

        assert_eq!(clear.id, None);
    }

    #[test]
    fn color_parses_rgb_and_rgba() {
        assert_eq!(
            "#336699".parse::<Color>().unwrap().to_rgba_tuple(),
            (
                0x33 as f64 / 255.0,
                0x66 as f64 / 255.0,
                0x99 as f64 / 255.0,
                1.0
            )
        );
        assert_eq!(
            "#336699cc".parse::<Color>().unwrap().to_rgba_tuple(),
            (
                0x33 as f64 / 255.0,
                0x66 as f64 / 255.0,
                0x99 as f64 / 255.0,
                0xcc as f64 / 255.0
            )
        );
    }

    #[test]
    fn color_serializes_to_protocol_hex() {
        let rgb = serde_json::to_value("#336699".parse::<Color>().unwrap()).unwrap();
        let rgba = serde_json::to_value("#336699cc".parse::<Color>().unwrap()).unwrap();

        assert_eq!(rgb, Value::String("#336699".to_string()));
        assert_eq!(rgba, Value::String("#336699cc".to_string()));
    }

    #[test]
    fn default_color_is_protocol_orange() {
        assert_eq!(
            serde_json::to_value(Color::default()).unwrap(),
            json!("#ffae00")
        );
        assert_eq!(
            Color::default().to_rgba_tuple(),
            (1.0, 174.0 / 255.0, 0.0, 1.0)
        );
    }

    #[test]
    fn invalid_color_fails_to_deserialize() {
        let err = serde_json::from_value::<Color>(json!("#12345")).unwrap_err();
        assert!(err.to_string().contains("#rrggbb"));
    }

    #[test]
    fn ttl_above_max_fails_to_deserialize() {
        let err = serde_json::from_value::<Message>(json!({
            "type": "draw_rect",
            "id": id(30),
            "ttl_ms": MAX_TTL_MS + 1,
            "x": 1.0,
            "y": 2.0,
            "w": 3.0,
            "h": 4.0
        }))
        .unwrap_err();

        assert!(err.to_string().contains("ttl_ms"));
    }

    #[test]
    fn label_above_max_chars_fails_to_deserialize() {
        let text = "x".repeat(MAX_LABEL_CHARS + 1);
        let err = serde_json::from_value::<Message>(json!({
            "type": "draw_label",
            "id": id(31),
            "x": 1.0,
            "y": 2.0,
            "text": text
        }))
        .unwrap_err();

        assert!(err.to_string().contains("text"));
    }

    #[test]
    fn label_with_control_chars_fails_to_deserialize() {
        let err = serde_json::from_value::<Message>(json!({
            "type": "draw_label",
            "id": id(32),
            "x": 1.0,
            "y": 2.0,
            "text": "line\nbreak"
        }))
        .unwrap_err();

        assert!(err.to_string().contains("control"));
    }

    #[test]
    fn coordinate_above_safe_bound_fails_to_deserialize() {
        let err = serde_json::from_value::<Message>(json!({
            "type": "draw_arrow",
            "id": id(33),
            "x1": 0.0,
            "y1": 0.0,
            "x2": MAX_COORD_ABS + 1.0,
            "y2": 4.0
        }))
        .unwrap_err();

        assert!(err.to_string().contains("coordinate"));
    }

    #[test]
    fn invalid_stroke_width_fails_to_deserialize() {
        let err = serde_json::from_value::<Message>(json!({
            "type": "draw_rect",
            "id": id(34),
            "stroke_width": MAX_STROKE_WIDTH + 1.0,
            "x": 1.0,
            "y": 2.0,
            "w": 3.0,
            "h": 4.0
        }))
        .unwrap_err();

        assert!(err.to_string().contains("stroke_width"));
    }

    #[test]
    fn round_trips_success_response() {
        let response = Response::ok(id(20));
        let encoded = serde_json::to_string(&response).unwrap();
        let decoded: Response = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, response);
    }

    #[test]
    fn round_trips_error_response() {
        let response = Response::error("invalid color");
        let encoded = serde_json::to_string(&response).unwrap();
        let decoded: Response = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, response);
    }

    fn assert_default_style(style: DrawStyle) {
        assert_eq!(style.ttl_ms, DEFAULT_TTL_MS);
        assert_eq!(style.color, Color::default());
        assert_eq!(style.stroke_width, DEFAULT_STROKE_WIDTH);
        assert_eq!(style.current_opacity, DEFAULT_CURRENT_OPACITY);
    }
}
