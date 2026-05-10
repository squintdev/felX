//! Asset references. Assets live on disk; the project file holds paths.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AssetId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
    Video,
    Image,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub id: AssetId,
    pub path: PathBuf,
    pub kind: AssetKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_construction() {
        let a = Asset {
            id: AssetId(1),
            path: PathBuf::from("media/clip.mp4"),
            kind: AssetKind::Video,
        };
        assert_eq!(a.id, AssetId(1));
        assert_eq!(a.kind, AssetKind::Video);
    }
}
