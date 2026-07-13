pub struct QuicPacketDetector;

impl QuicPacketDetector {
    pub fn is_quic_packet(data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }

        let first_byte = data[0];
        let header_form = (first_byte >> 7) & 0x01;

        if header_form == 1 {
            let long_packet_type = (first_byte >> 4) & 0x03;
            matches!(long_packet_type, 0..=3)
        } else {
            first_byte & 0x40 != 0 && data.len() >= 20
        }
    }

    pub fn is_long_header(data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }
        data[0] & 0x80 != 0
    }

    pub fn is_short_header(data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }
        data[0] & 0x80 == 0 && data[0] & 0x40 != 0
    }

    pub fn get_long_packet_type(data: &[u8]) -> Option<QuicLongPacketType> {
        if !Self::is_long_header(data) {
            return None;
        }

        let packet_type = (data[0] >> 4) & 0x03;
        match packet_type {
            0 => Some(QuicLongPacketType::Initial),
            1 => Some(QuicLongPacketType::ZeroRtt),
            2 => Some(QuicLongPacketType::Handshake),
            3 => Some(QuicLongPacketType::Retry),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuicLongPacketType {
    Initial = 0,
    ZeroRtt = 1,
    Handshake = 2,
    Retry = 3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_data() {
        assert!(!QuicPacketDetector::is_quic_packet(&[]));
        assert!(!QuicPacketDetector::is_long_header(&[]));
        assert!(!QuicPacketDetector::is_short_header(&[]));
    }

    #[test]
    fn test_long_header_initial() {
        let mut packet = vec![0xc0]; // Long header + Initial type
        packet.extend_from_slice(&[0x00; 30]);
        assert!(QuicPacketDetector::is_quic_packet(&packet));
        assert!(QuicPacketDetector::is_long_header(&packet));
        assert!(!QuicPacketDetector::is_short_header(&packet));
        assert_eq!(
            QuicPacketDetector::get_long_packet_type(&packet),
            Some(QuicLongPacketType::Initial)
        );
    }

    #[test]
    fn test_long_header_handshake() {
        let mut packet = vec![0xe0]; // Long header + Handshake type
        packet.extend_from_slice(&[0x00; 30]);
        assert!(QuicPacketDetector::is_quic_packet(&packet));
        assert_eq!(
            QuicPacketDetector::get_long_packet_type(&packet),
            Some(QuicLongPacketType::Handshake)
        );
    }

    #[test]
    fn coverage_90_all_long_packet_types_and_short_inputs() {
        assert_eq!(QuicPacketDetector::get_long_packet_type(&[]), None);
        assert_eq!(QuicPacketDetector::get_long_packet_type(&[0x40]), None);
        for (bits, expected) in [
            (0_u8, QuicLongPacketType::Initial),
            (1, QuicLongPacketType::ZeroRtt),
            (2, QuicLongPacketType::Handshake),
            (3, QuicLongPacketType::Retry),
        ] {
            let mut packet = vec![0x80 | (bits << 4), 0, 0, 0, 1];
            packet.extend_from_slice(&[0; 8]);
            assert_eq!(
                QuicPacketDetector::get_long_packet_type(&packet),
                Some(expected)
            );
        }
    }

    #[test]
    fn test_short_header() {
        let mut packet = vec![0x40]; // Short header (fixed bit set)
        packet.extend_from_slice(&[0x00; 25]);
        assert!(QuicPacketDetector::is_quic_packet(&packet));
        assert!(!QuicPacketDetector::is_long_header(&packet));
        assert!(QuicPacketDetector::is_short_header(&packet));
    }

    #[test]
    fn test_short_header_too_small() {
        let packet = vec![0x40; 10];
        assert!(!QuicPacketDetector::is_quic_packet(&packet));
    }

    #[test]
    fn test_non_quic_packet() {
        let packet = vec![0x00, 0x01, 0x02, 0x03];
        assert!(!QuicPacketDetector::is_quic_packet(&packet));
    }
}
