use chrono::{DateTime, Datelike, Timelike, Utc};

/// State reported by the virtual GPS receiver.
#[derive(Clone, Debug, PartialEq)]
pub struct GpsState {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: f64,
    pub speed_knots: f64,
    pub course_deg: f64,
    pub satellites: u8,
    pub hdop: f64,
    /// 0 = no fix, 1 = GPS, 2 = DGPS.
    pub fix_quality: u8,
    pub enabled: bool,
    /// When set, NMEA sentences use this timestamp instead of `Utc::now()`.
    /// Enables deterministic scenario replay.
    fixed_time: Option<DateTime<Utc>>,
}

impl GpsState {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            latitude: lat,
            longitude: lon,
            altitude_m: 0.0,
            speed_knots: 0.0,
            course_deg: 0.0,
            satellites: 8,
            hdop: 0.9,
            fix_quality: 1,
            enabled: true,
            fixed_time: None,
        }
    }

    /// Pin the GPS clock to a specific Unix timestamp for deterministic replay.
    ///
    /// Pass `seconds == 0` to clear the pin and fall back to `Utc::now()`.
    pub fn set_fixed_time(&mut self, unix_seconds: Option<i64>) {
        self.fixed_time = unix_seconds
            .filter(|&s| s != 0)
            .and_then(|s| DateTime::from_timestamp(s, 0));
    }

    fn now(&self) -> DateTime<Utc> {
        self.fixed_time.unwrap_or_else(Utc::now)
    }

    /// Generate a `$GPGGA` fix-data sentence.
    pub fn generate_gga(&self) -> String {
        self.generate_gga_at(self.now())
    }

    /// Generate a `$GPRMC` recommended-minimum-data sentence.
    pub fn generate_rmc(&self) -> String {
        self.generate_rmc_at(self.now())
    }

    /// Generate a `$GPGSA` dilution-of-precision sentence.
    pub fn generate_gsa(&self) -> String {
        let fix_type = if self.enabled && self.fix_quality > 0 {
            3
        } else {
            1
        };
        let mut satellite_fields = Vec::with_capacity(12);
        for prn in 1..=12 {
            if prn <= self.satellites.min(12) {
                satellite_fields.push(format!("{prn:02}"));
            } else {
                satellite_fields.push(String::new());
            }
        }
        let pdop = self.hdop * 1.5;
        let vdop = self.hdop * 1.2;
        let body = format!(
            "GPGSA,A,{fix_type},{},{pdop:.1},{:.1},{vdop:.1}",
            satellite_fields.join(","),
            self.hdop
        );
        finish_sentence(body)
    }

    /// Generate the complete set of `$GPGSV` satellites-in-view sentences.
    ///
    /// A GSV sentence can describe at most four satellites, so a complete
    /// view may contain multiple ordered fragments.
    pub fn generate_gsv(&self) -> Vec<String> {
        let satellites = self.satellites;
        let message_count = satellites.max(1).div_ceil(4);
        (0..message_count)
            .map(|fragment| {
                let message_number = fragment + 1;
                let first_prn = fragment.saturating_mul(4).saturating_add(1);
                let last_prn = first_prn.saturating_add(3).min(satellites);
                let mut body = format!("GPGSV,{message_count},{message_number},{satellites:02}");
                for prn in first_prn..=last_prn {
                    let elevation = 30 + u16::from(prn) * 5;
                    let azimuth = (u16::from(prn) * 67) % 360;
                    let snr = 35 + u16::from(prn);
                    body.push_str(&format!(",{prn:02},{elevation:02},{azimuth:03},{snr:02}"));
                }
                finish_sentence(body)
            })
            .collect()
    }

    pub(crate) fn generate_gga_at(&self, now: DateTime<Utc>) -> String {
        let (lat_deg, lat_min, lat_dir, lon_deg, lon_min, lon_dir) = self.coordinate_fields();
        let fix_quality = if self.enabled { self.fix_quality } else { 0 };
        let satellites = if fix_quality == 0 { 0 } else { self.satellites };
        let body = format!(
            "GPGGA,{:02}{:02}{:02},{lat_deg:02}{lat_min:07.4},{lat_dir},\
             {lon_deg:03}{lon_min:07.4},{lon_dir},{fix_quality},{satellites:02},\
             {:.1},{:.1},M,0.0,M,,",
            now.hour(),
            now.minute(),
            now.second(),
            self.hdop,
            self.altitude_m
        )
        .replace(' ', "");
        finish_sentence(body)
    }

    pub(crate) fn generate_rmc_at(&self, now: DateTime<Utc>) -> String {
        let (lat_deg, lat_min, lat_dir, lon_deg, lon_min, lon_dir) = self.coordinate_fields();
        let status = if self.enabled && self.fix_quality > 0 {
            "A"
        } else {
            "V"
        };
        let body = format!(
            "GPRMC,{:02}{:02}{:02},{status},{lat_deg:02}{lat_min:07.4},{lat_dir},\
             {lon_deg:03}{lon_min:07.4},{lon_dir},{:.1},{:.1},{:02}{:02}{:02},,,A",
            now.hour(),
            now.minute(),
            now.second(),
            self.speed_knots,
            self.course_deg,
            now.day(),
            now.month(),
            now.year().rem_euclid(100)
        )
        .replace(' ', "");
        finish_sentence(body)
    }

    fn coordinate_fields(&self) -> (u32, f64, &'static str, u32, f64, &'static str) {
        let lat_deg = self.latitude.abs() as u32;
        let lat_min = (self.latitude.abs() - f64::from(lat_deg)) * 60.0;
        let lat_dir = if self.latitude >= 0.0 { "N" } else { "S" };
        let lon_deg = self.longitude.abs() as u32;
        let lon_min = (self.longitude.abs() - f64::from(lon_deg)) * 60.0;
        let lon_dir = if self.longitude >= 0.0 { "E" } else { "W" };
        (lat_deg, lat_min, lat_dir, lon_deg, lon_min, lon_dir)
    }
}

fn finish_sentence(body: String) -> String {
    let checksum = calculate_checksum(&body);
    format!("${body}*{checksum:02X}\r\n")
}

fn calculate_checksum(sentence: &str) -> u8 {
    sentence.bytes().fold(0_u8, |acc, byte| acc ^ byte)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveTime};

    use super::*;

    fn fixed_time() -> DateTime<Utc> {
        let date = NaiveDate::from_ymd_opt(1994, 3, 23).unwrap();
        let time = NaiveTime::from_hms_opt(12, 35, 19).unwrap();
        DateTime::from_naive_utc_and_offset(date.and_time(time), Utc)
    }

    fn assert_valid_checksum(sentence: &str) {
        let sentence = sentence
            .strip_prefix('$')
            .unwrap()
            .strip_suffix("\r\n")
            .unwrap();
        let (body, checksum) = sentence.rsplit_once('*').unwrap();
        assert_eq!(
            calculate_checksum(body),
            u8::from_str_radix(checksum, 16).unwrap()
        );
    }

    #[test]
    fn generates_gga_for_known_coordinates() {
        let mut state = GpsState::new(48.1173, 11.5166667);
        state.altitude_m = 545.4;
        state.satellites = 8;
        state.hdop = 0.9;

        let sentence = state.generate_gga_at(fixed_time());

        assert_eq!(
            sentence,
            "$GPGGA,123519,4807.0380,N,01131.0000,E,1,08,0.9,545.4,M,0.0,M,,*7C\r\n"
        );
        assert_valid_checksum(&sentence);
    }

    #[test]
    fn generates_rmc_for_known_coordinates() {
        let mut state = GpsState::new(48.1173, 11.5166667);
        state.speed_knots = 22.4;
        state.course_deg = 84.4;

        let sentence = state.generate_rmc_at(fixed_time());

        assert_eq!(
            sentence,
            "$GPRMC,123519,A,4807.0380,N,01131.0000,E,22.4,84.4,230394,,,A*7C\r\n"
        );
        assert_valid_checksum(&sentence);
    }

    #[test]
    fn calculates_standard_nmea_checksum() {
        assert_eq!(
            calculate_checksum("GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,"),
            0x47
        );
    }

    #[test]
    fn generates_complete_gsv_vector_for_eight_satellites() {
        let state = GpsState::new(48.1173, 11.5166667);

        let sentences = state.generate_gsv();

        assert_eq!(
            sentences,
            [
                "$GPGSV,2,1,08,01,35,067,36,02,40,134,37,03,45,201,38,04,50,268,39*78\r\n",
                "$GPGSV,2,2,08,05,55,335,40,06,60,042,41,07,65,109,42,08,70,176,43*74\r\n",
            ]
        );
        sentences
            .iter()
            .for_each(|sentence| assert_valid_checksum(sentence));
    }

    #[test]
    fn generates_one_empty_gsv_fragment_when_no_satellites_are_visible() {
        let mut state = GpsState::new(48.1173, 11.5166667);
        state.satellites = 0;

        assert_eq!(state.generate_gsv(), ["$GPGSV,1,1,00*79\r\n"]);
    }
}
