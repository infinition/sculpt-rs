//! One vertex attribute, stored on its own and only once it differs.
//!
//! A vertex used to be one struct of twelve floats: position, normal, colour,
//! mask, roughness, metalness, interleaved. Every pass dragged all forty-eight
//! bytes through the cache even when it read three of them, and recomputing the
//! normals after a dab, which reads positions and writes normals, paid for the
//! colour of every vertex it touched.
//!
//! Each attribute now lives in its own array, quantised to what it actually
//! needs rather than to what the widest of them needs. And an attribute nobody
//! has touched is **dormant**: it holds no memory at all and answers its default
//! to every read. A model that has never been painted carries no colour array,
//! no roughness, no metalness and no mask, which is most models for most of
//! their life.
//!
//! Waking is automatic and irreversible within a session: the first write that
//! differs from the default allocates the array, filled with that default, and
//! from then on the channel is ordinary storage. Nothing asks whether a channel
//! is awake before reading it.

/// An attribute of every vertex, or the absence of one.
///
/// `T` is the stored form, already quantised. The conversion to and from the
/// numbers the rest of the engine works in belongs to the caller, because only
/// the caller knows whether a byte means a colour or a roughness.
#[derive(Clone, Debug)]
pub struct Channel<T: Copy + PartialEq> {
    /// `None` while every entry is still `fill`.
    data: Option<Vec<T>>,
    /// What a dormant channel answers, and what a waking one is filled with.
    fill: T,
    /// Entries, whether or not any are stored.
    len: usize,
}

impl<T: Copy + PartialEq> Channel<T> {
    pub fn new(fill: T) -> Self {
        Self { data: None, fill, len: 0 }
    }

    pub fn with_len(fill: T, len: usize) -> Self {
        Self { data: None, fill, len }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True while this channel holds no memory.
    #[inline]
    pub fn is_dormant(&self) -> bool {
        self.data.is_none()
    }

    /// What a read answers when nothing has been written.
    #[inline]
    pub fn fill(&self) -> T {
        self.fill
    }

    #[inline]
    pub fn get(&self, i: usize) -> T {
        match &self.data {
            Some(v) => v[i],
            None => self.fill,
        }
    }

    /// Writing the default into a dormant channel leaves it dormant, which is
    /// what keeps a mesh that is painted white from allocating a colour array.
    #[inline]
    pub fn set(&mut self, i: usize, value: T) {
        match &mut self.data {
            Some(v) => v[i] = value,
            None => {
                if value == self.fill {
                    return;
                }
                self.wake();
                self.data.as_mut().expect("just woken")[i] = value;
            }
        }
    }

    /// The stored slice, or nothing while dormant.
    ///
    /// For the passes that want to walk an attribute rather than ask for it one
    /// entry at a time. A dormant channel answers `None` and the caller uses the
    /// default for every entry, which is faster than reading an array of them.
    #[inline]
    pub fn as_slice(&self) -> Option<&[T]> {
        self.data.as_deref()
    }

    /// Allocates if needed and hands over the storage.
    ///
    /// For a pass that is about to write most of the channel anyway, where
    /// checking each value against the default first would cost more than the
    /// allocation.
    pub fn make_mut(&mut self) -> &mut [T] {
        if self.data.is_none() {
            self.wake();
        }
        self.data.as_mut().expect("just woken")
    }

    pub fn push(&mut self, value: T) {
        self.len += 1;
        match &mut self.data {
            Some(v) => v.push(value),
            None => {
                if value != self.fill {
                    // Wake at the length before this push, then append.
                    self.len -= 1;
                    self.wake();
                    self.len += 1;
                    self.data.as_mut().expect("just woken").push(value);
                }
            }
        }
    }

    /// Mirrors `Vec::swap_remove`, and stays dormant if it was.
    pub fn swap_remove(&mut self, i: usize) {
        debug_assert!(i < self.len);
        self.len -= 1;
        if let Some(v) = &mut self.data {
            v.swap_remove(i);
        }
    }

    pub fn truncate(&mut self, n: usize) {
        if n >= self.len {
            return;
        }
        self.len = n;
        if let Some(v) = &mut self.data {
            v.truncate(n);
        }
    }

    /// Grows with the default, or shrinks. Stays dormant either way.
    pub fn resize(&mut self, n: usize) {
        if n < self.len {
            self.truncate(n);
            return;
        }
        if let Some(v) = &mut self.data {
            v.resize(n, self.fill);
        }
        self.len = n;
    }

    /// Empties the channel and puts it back to sleep.
    pub fn clear(&mut self) {
        self.data = None;
        self.len = 0;
    }

    /// Drops the storage, which is only correct when every entry is the
    /// default. Used after an operation that has just written the default
    /// everywhere.
    pub fn sleep_if_uniform(&mut self) {
        let uniform = match &self.data {
            Some(v) => v.iter().all(|x| *x == self.fill),
            None => return,
        };
        if uniform {
            self.data = None;
        }
    }

    /// What this channel costs in memory right now.
    pub fn bytes(&self) -> usize {
        match &self.data {
            Some(v) => v.len() * std::mem::size_of::<T>(),
            None => 0,
        }
    }

    fn wake(&mut self) {
        self.data = Some(vec![self.fill; self.len]);
    }
}

impl<T: Copy + PartialEq> Default for Channel<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

// ---------------------------------------------------------------------------
// Quantisation
// ---------------------------------------------------------------------------

/// A number in zero to one, as a byte.
///
/// Two hundred and fifty six steps is what a colour picker offers and what a
/// texture would have held, so nothing is lost that anyone was going to see.
#[inline]
pub fn to_unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[inline]
pub fn from_unorm8(v: u8) -> f32 {
    v as f32 * (1.0 / 255.0)
}

/// A number in zero to one, as two bytes.
///
/// The mask is a blend factor that brushes read and write repeatedly, so it
/// keeps more room than a colour does.
#[inline]
pub fn to_unorm16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

#[inline]
pub fn from_unorm16(v: u16) -> f32 {
    v as f32 * (1.0 / 65535.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_nobody_writes_to_holds_nothing() {
        let mut c: Channel<u8> = Channel::new(153);
        for _ in 0..1000 {
            c.push(153);
        }
        assert!(c.is_dormant(), "the channel woke for its own default");
        assert_eq!(c.bytes(), 0);
        assert_eq!(c.len(), 1000);
        assert_eq!(c.get(500), 153, "a dormant channel must answer its default");
    }

    #[test]
    fn writing_the_default_does_not_wake_it() {
        let mut c: Channel<u8> = Channel::with_len(153, 10);
        c.set(3, 153);
        assert!(c.is_dormant());
        c.set(3, 200);
        assert!(!c.is_dormant(), "a real write must wake it");
        assert_eq!(c.get(3), 200);
        assert_eq!(c.get(4), 153, "waking must fill the rest with the default");
    }

    /// A push that differs has to wake the channel and keep every entry that
    /// came before it.
    #[test]
    fn waking_on_a_push_keeps_the_history() {
        let mut c: Channel<u8> = Channel::new(0);
        for _ in 0..5 {
            c.push(0);
        }
        c.push(42);
        assert!(!c.is_dormant());
        assert_eq!(c.len(), 6);
        for i in 0..5 {
            assert_eq!(c.get(i), 0, "entry {i} was lost when the channel woke");
        }
        assert_eq!(c.get(5), 42);
    }

    /// Removal has to behave the same asleep and awake, since the mesh renumbers
    /// vertices constantly and cannot ask which state a channel is in.
    #[test]
    fn removal_matches_whether_it_is_awake_or_not() {
        let mut asleep: Channel<u8> = Channel::with_len(7, 0);
        let mut awake: Channel<u8> = Channel::with_len(7, 0);
        awake.set(0, 7);
        for v in [7u8, 7, 7, 7] {
            asleep.push(v);
            awake.push(v);
        }
        awake.make_mut();
        assert!(asleep.is_dormant() && !awake.is_dormant());

        asleep.swap_remove(1);
        awake.swap_remove(1);
        assert_eq!(asleep.len(), awake.len());
        for i in 0..asleep.len() {
            assert_eq!(asleep.get(i), awake.get(i), "entry {i}");
        }
    }

    #[test]
    fn resize_grows_with_the_default() {
        let mut c: Channel<u16> = Channel::new(0);
        c.push(500);
        c.resize(4);
        assert_eq!(c.len(), 4);
        assert_eq!(c.get(0), 500);
        assert_eq!(c.get(3), 0);
        c.resize(2);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn a_channel_written_back_to_its_default_can_sleep_again() {
        let mut c: Channel<u8> = Channel::with_len(0, 100);
        c.set(50, 9);
        assert!(!c.is_dormant());
        c.set(50, 0);
        c.sleep_if_uniform();
        assert!(c.is_dormant(), "clearing a mask should give the memory back");
    }

    /// Quantisation has to survive the trip at the precision the engine works
    /// in, or painting drifts every time a value is read back.
    #[test]
    fn quantisation_round_trips_closely_enough() {
        for i in 0..=255u32 {
            let v = i as f32 / 255.0;
            assert!((from_unorm8(to_unorm8(v)) - v).abs() < 1.0 / 400.0, "{v}");
        }
        for i in 0..=1000u32 {
            let v = i as f32 / 1000.0;
            assert!((from_unorm16(to_unorm16(v)) - v).abs() < 1.0 / 60000.0, "{v}");
        }
        assert_eq!(to_unorm8(-5.0), 0, "out of range must clamp, not wrap");
        assert_eq!(to_unorm8(5.0), 255);
        assert_eq!(to_unorm16(5.0), 65535);
    }
}
