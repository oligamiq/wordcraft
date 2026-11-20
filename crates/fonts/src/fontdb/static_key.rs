//! Value identity for a build-time static font face, independent of process IDs and pointers.
//! The digest identifies the entire font file; collection index and normalized variation
//! coordinates identify a face within that file. This is not an authorization token.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StaticFaceKey {
    sha256: [u8; 32],
    index: u32,
    axes: Vec<([u8; 4], u32)>,
}

impl StaticFaceKey {
    pub fn new(sha256: [u8; 32], index: u32, coords: &[([u8; 4], f32)]) -> Option<Self> {
        let mut axes = Vec::with_capacity(coords.len());
        for &(tag, value) in coords {
            if !value.is_finite() {
                return None;
            }
            axes.push((tag, if value == 0.0 { 0 } else { value.to_bits() }));
        }
        axes.sort_unstable_by_key(|(tag, _)| *tag);
        if axes.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return None;
        }
        Some(Self { sha256, index, axes })
    }

    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn axes(&self) -> &[([u8; 4], u32)] {
        &self.axes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_key_tracks_file_collection_member_and_variations() {
        let base = StaticFaceKey::new([7; 32], 0, &[]).unwrap();
        assert_ne!(base, StaticFaceKey::new([8; 32], 0, &[]).unwrap());
        assert_ne!(base, StaticFaceKey::new([7; 32], 1, &[]).unwrap());
        assert_ne!(base, StaticFaceKey::new([7; 32], 0, &[(*b"wght", 400.0)]).unwrap());
        assert_ne!(
            StaticFaceKey::new([7; 32], 0, &[(*b"wght", 400.0)]),
            StaticFaceKey::new([7; 32], 0, &[(*b"wght", 700.0)])
        );
    }

    #[test]
    fn static_key_is_canonical_and_rejects_invalid_axes() {
        let a = StaticFaceKey::new([7; 32], 2, &[(*b"wght", 400.0), (*b"slnt", -0.0)]).unwrap();
        let b = StaticFaceKey::new([7; 32], 2, &[(*b"slnt", 0.0), (*b"wght", 400.0)]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.sha256(), &[7; 32]);
        assert_eq!(a.index(), 2);
        assert_eq!(a.axes(), &[(*b"slnt", 0), (*b"wght", 400.0f32.to_bits())]);
        assert!(StaticFaceKey::new([7; 32], 0, &[(*b"wght", f32::NAN)]).is_none());
        assert!(StaticFaceKey::new([7; 32], 0, &[(*b"wght", f32::INFINITY)]).is_none());
        assert!(StaticFaceKey::new([7; 32], 0, &[(*b"wght", 400.0), (*b"wght", 700.0)]).is_none());
    }
}
