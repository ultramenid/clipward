//! The seek index at the start of a WebM file: where each cluster begins, by time and by
//! byte. With it a download can fetch only the clusters a time range needs. The bytes
//! come from the network, so every read is bounds-checked and anything unexpected is None.

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const CUES: u32 = 0x1C53_BB6B;
const CUE_POINT: u32 = 0xBB;
const CUE_TIME: u32 = 0xB3;
const CUE_TRACK_POSITIONS: u32 = 0xB7;
const CUE_CLUSTER_POSITION: u32 = 0xF1;
const CLUSTER: u32 = 0x1F43_B675;

#[derive(Debug, PartialEq)]
pub struct WebmIndex {
    /// Bytes before the first cluster: the EBML header, track info and the cues.
    pub header_len: u64,
    /// Indexed clusters, ascending: start time in seconds and byte offset in the file.
    pub clusters: Vec<(f64, u64)>,
}

/// The element header at `pos`: id, where its body starts, and the body's length
/// (None when the file leaves it open, as a live Segment does).
fn element(data: &[u8], pos: usize) -> Option<(u32, usize, Option<usize>)> {
    let id_len = data.get(pos)?.leading_zeros() as usize + 1;
    if id_len > 4 {
        return None;
    }
    let size_pos = pos.checked_add(id_len)?;
    let id = data.get(pos..size_pos)?.iter().fold(0u32, |id, b| (id << 8) | u32::from(*b));
    let first = *data.get(size_pos)?;
    let size_len = first.leading_zeros() as usize + 1;
    let body = size_pos.checked_add(size_len)?;
    // An all-zero first byte (size_len 9) is not a valid length.
    let marker = 0x80u8.checked_shr(size_len as u32 - 1)?;
    let rest = data.get(size_pos + 1..body)?;
    let size = rest.iter().fold(u64::from(first ^ marker), |size, b| (size << 8) | u64::from(*b));
    let open = size == (1u64 << (7 * size_len)) - 1;
    Some((id, body, if open { None } else { Some(usize::try_from(size).ok()?) }))
}

/// The elements directly inside `data`, each wholly within it.
fn children(data: &[u8]) -> Option<Vec<(u32, &[u8])>> {
    let (mut pos, mut found) = (0, Vec::new());
    while pos < data.len() {
        let (id, body, len) = element(data, pos)?;
        let end = body.checked_add(len?)?;
        found.push((id, data.get(body..end)?));
        pos = end;
    }
    Some(found)
}

fn uint(data: &[u8]) -> Option<u64> {
    (data.len() <= 8).then(|| data.iter().fold(0, |value, b| (value << 8) | u64::from(*b)))
}

fn child(data: &[u8], id: u32) -> Option<&[u8]> {
    children(data)?.into_iter().find(|(found, _)| *found == id).map(|(_, body)| body)
}

impl WebmIndex {
    /// Reads the index from the first bytes of a file. `head` must reach the first
    /// cluster, and the cues must come before it (as in YouTube's DASH files).
    pub fn parse(head: &[u8]) -> Option<Self> {
        let (id, body, len) = element(head, 0)?;
        if id != EBML {
            return None;
        }
        let (id, segment, _) = element(head, body.checked_add(len?)?)?;
        if id != SEGMENT {
            return None;
        }
        let (mut pos, mut scale, mut cues) = (segment, 1_000_000u64, Vec::new());
        let header_len = loop {
            let (id, body, len) = element(head, pos)?;
            if id == CLUSTER {
                break pos;
            }
            let end = body.checked_add(len?)?;
            let data = head.get(body..end)?;
            if id == INFO {
                if let Some(value) = child(data, TIMECODE_SCALE) {
                    scale = uint(value)?;
                }
            } else if id == CUES {
                for (_, point) in children(data)?.into_iter().filter(|(id, _)| *id == CUE_POINT) {
                    let time = uint(child(point, CUE_TIME)?)?;
                    let position = uint(child(child(point, CUE_TRACK_POSITIONS)?, CUE_CLUSTER_POSITION)?)?;
                    cues.push((time, position));
                }
            }
            pos = end;
        };
        let mut clusters: Vec<(f64, u64)> = Vec::with_capacity(cues.len());
        for (time, position) in cues {
            let offset = (segment as u64).checked_add(position)?;
            let seconds = time as f64 * scale as f64 / 1e9;
            // Several cues can point into one cluster; a file out of order is not trusted.
            match clusters.last() {
                Some((_, last)) if offset == *last => continue,
                Some((at, last)) if offset < *last || seconds < *at => return None,
                None if offset < header_len as u64 => return None,
                _ => clusters.push((seconds, offset)),
            }
        }
        (!clusters.is_empty()).then_some(Self { header_len: header_len as u64, clusters })
    }

    /// Inclusive byte ranges of every cluster that overlaps `spans` (seconds) in a file
    /// of `file_len` bytes, ascending and merged.
    pub fn byte_ranges(&self, spans: &[(f64, f64)], file_len: u64) -> Vec<(u64, u64)> {
        let mut ranges: Vec<(u64, u64)> = spans
            .iter()
            .filter_map(|(from, to)| {
                let start = self.clusters.iter().rev().find(|c| c.0 <= *from).map_or(self.header_len, |c| c.1);
                let end = self.clusters.iter().find(|c| c.0 >= *to && c.1 > start).map_or(file_len, |c| c.1);
                (end > start).then_some((start, end - 1))
            })
            .collect();
        ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (start, end) in ranges {
            match merged.last_mut() {
                Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        merged
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An element with a one- or two-byte length.
    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        if body.len() < 127 {
            out.push(0x80 | body.len() as u8);
        } else {
            out.extend([0x40 | (body.len() >> 8) as u8, body.len() as u8]);
        }
        out.extend(body);
        out
    }

    fn cue(time: u16, position: u16) -> Vec<u8> {
        let track = el(&[0xF7], &[1]);
        let at = el(&[0xF1], &position.to_be_bytes());
        el(&[0xBB], &[el(&[0xB3], &time.to_be_bytes()), el(&[0xB7], &[track, at].concat())].concat())
    }

    /// EBML header, then a Segment of open length: info, tracks, cues and the first cluster.
    /// Returns the bytes, where the Segment's body starts and where the first cluster does.
    pub(crate) fn head(cues: &[(u16, u16)], scale: Option<u32>) -> (Vec<u8>, u64, u64) {
        let ebml = el(&[0x1A, 0x45, 0xDF, 0xA3], &[0x42, 0x82, 0x84, b'w', b'e', b'b', b'm']);
        let info = el(&[0x15, 0x49, 0xA9, 0x66], &scale.map_or(Vec::new(), |s| el(&[0x2A, 0xD7, 0xB1], &s.to_be_bytes()[1..])));
        let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &[0u8; 200]);
        let cues = el(&[0x1C, 0x53, 0xBB, 0x6B], &cues.iter().flat_map(|(t, p)| cue(*t, *p)).collect::<Vec<_>>());
        let mut out = [ebml, vec![0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]].concat();
        let segment = out.len() as u64;
        out.extend([info, tracks, cues].concat());
        let header_len = out.len() as u64;
        out.extend([0x1F, 0x43, 0xB6, 0x75, 0x88, 0, 0]);
        (out, segment, header_len)
    }

    #[test]
    fn reads_cluster_times_and_offsets_before_the_first_cluster() {
        // Positions count from the Segment's first byte; times are in milliseconds by default.
        let (bytes, segment, header_len) = head(&[(0, 400), (4000, 900), (4100, 900), (9500, 2000)], None);
        let index = WebmIndex::parse(&bytes).unwrap();
        assert_eq!(index.header_len, header_len);
        assert_eq!(index.clusters, [(0.0, segment + 400), (4.0, segment + 900), (9.5, segment + 2000)]);
        // A timecode scale of 2 ms doubles every time.
        let (bytes, ..) = head(&[(0, 400), (4000, 900)], Some(2_000_000));
        assert_eq!(WebmIndex::parse(&bytes).unwrap().clusters[1].0, 8.0);
    }

    #[test]
    fn rejects_anything_that_is_not_a_complete_ordered_index() {
        let (bytes, ..) = head(&[(0, 400), (4000, 900)], None);
        // Cut anywhere before the first cluster: the index is incomplete.
        for len in 0..bytes.len() - 7 {
            assert_eq!(WebmIndex::parse(&bytes[..len]), None, "{len}");
        }
        assert_eq!(WebmIndex::parse(&head(&[], None).0), None);
        assert_eq!(WebmIndex::parse(&head(&[(0, 900), (4000, 400)], None).0), None);
        assert_eq!(WebmIndex::parse(&head(&[(4000, 400), (0, 900)], None).0), None);
        assert_eq!(WebmIndex::parse(&head(&[(0, 3)], None).0), None);
        assert_eq!(WebmIndex::parse(b"\x00\x00\x00\x20ftypisom"), None);
        assert_eq!(WebmIndex::parse(&[0xFF; 64]), None);
    }

    #[test]
    fn byte_ranges_cover_whole_clusters_and_merge() {
        let index = WebmIndex { header_len: 100, clusters: vec![(0.0, 100), (4.0, 500), (8.0, 900), (12.0, 1500)] };
        // 5-7 s lies in the cluster that starts at 4 s.
        assert_eq!(index.byte_ranges(&[(5.0, 7.0)], 2000), [(500, 899)]);
        // A span ending exactly on a cluster start does not need that cluster.
        assert_eq!(index.byte_ranges(&[(4.0, 8.0)], 2000), [(500, 899)]);
        // Past the last cluster runs to the end of the file; before the first starts at the header.
        assert_eq!(index.byte_ranges(&[(13.0, 99.0)], 2000), [(1500, 1999)]);
        assert_eq!(index.byte_ranges(&[(-3.0, 1.0)], 2000), [(100, 499)]);
        // Unsorted, touching and overlapping spans come back ascending and merged.
        assert_eq!(index.byte_ranges(&[(13.0, 14.0), (1.0, 2.0), (5.0, 9.0), (6.0, 7.0)], 2000), [(100, 1999)]);
        assert_eq!(index.byte_ranges(&[(13.0, 14.0), (1.0, 2.0)], 2000), [(100, 499), (1500, 1999)]);
        assert_eq!(index.byte_ranges(&[(7.0, 5.0)], 2000), [(500, 899)]);
        assert!(index.byte_ranges(&[], 2000).is_empty());
    }
}
