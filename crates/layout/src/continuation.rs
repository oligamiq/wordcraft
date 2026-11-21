//! Experimental source-bound transport envelope for future durable layout continuation.
//! The payload is not yet a serializable pagination state; do not use it to resume layout.

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ContinuationEnvelope {
        ContinuationEnvelope {
            source_digest: [1; 32],
            options_digest: [2; 32],
            cursor: Cursor { phase: Phase::Paginating, iteration: 2, section: 1, block: 4 },
            payload: vec![9, 8, 7],
        }
    }

    #[test]
    fn envelope_roundtrip() {
        let envelope = fixture();
        let bytes = encode(&envelope).unwrap();
        assert_eq!(decode(&bytes, [1; 32], [2; 32], 3, 10).unwrap(), envelope);
    }

    #[test]
    fn wrong_source_and_options_are_rejected() {
        let bytes = encode(&fixture()).unwrap();
        assert_eq!(decode(&bytes, [3; 32], [2; 32], 3, 10), Err(ContinuationError::WrongSource));
        assert_eq!(decode(&bytes, [1; 32], [3; 32], 3, 10), Err(ContinuationError::WrongOptions));
    }

    #[test]
    fn corrupted_version_phase_and_payload_fail_closed() {
        let bytes = encode(&fixture()).unwrap();
        let mut version = bytes.clone();
        version[4] = 44;
        assert_eq!(decode(&version, [1; 32], [2; 32], 3, 10), Err(ContinuationError::WrongVersion));
        let mut phase = bytes.clone();
        phase[5] = 127;
        assert_eq!(decode(&phase, [1; 32], [2; 32], 3, 10), Err(ContinuationError::InvalidPhase));
        let mut altered = bytes.clone();
        *altered.last_mut().unwrap() ^= 1;
        assert_eq!(decode(&altered, [1; 32], [2; 32], 3, 10), Err(ContinuationError::Integrity));
    }

    #[test]
    fn truncated_extra_and_invalid_cursor_fail_closed() {
        let bytes = encode(&fixture()).unwrap();
        assert_eq!(decode(&bytes[..bytes.len() - 1], [1; 32], [2; 32], 3, 10), Err(ContinuationError::Malformed));
        let mut extended = bytes.clone();
        extended.push(0);
        assert_eq!(decode(&extended, [1; 32], [2; 32], 3, 10), Err(ContinuationError::Malformed));
        assert_eq!(decode(&bytes, [1; 32], [2; 32], 0, 10), Err(ContinuationError::OutOfBounds));
        assert_eq!(decode(&bytes, [1; 32], [2; 32], 3, 3), Err(ContinuationError::OutOfBounds));
    }

    #[test]
    fn empty_or_oversized_payload_is_never_emitted() {
        let mut envelope = fixture();
        envelope.payload.clear();
        assert_eq!(encode(&envelope), Err(ContinuationError::PayloadSize));
        envelope.payload.resize(MAX_PAYLOAD + 1, 7);
        assert_eq!(encode(&envelope), Err(ContinuationError::PayloadSize));
    }

    #[test]
    fn invalid_encoding_cursor_is_never_emitted() {
        let mut envelope = fixture();
        envelope.cursor.iteration = u32::MAX;
        assert_eq!(encode(&envelope), Err(ContinuationError::OutOfBounds));
    }
}
