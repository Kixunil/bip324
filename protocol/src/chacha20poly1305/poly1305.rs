// SPDX-License-Identifier: MIT OR Apache-2.0

//! Poly1305 one-time authenticator from RFC7539.
//!
//! Implementation heavily inspired by [this implementation in C](https://github.com/floodyberry/poly1305-donna/blob/master/poly1305-donna-32.h)
//! referred to as "Donna". Further reference to [this](https://loup-vaillant.fr/tutorials/poly1305-design) article was used to formulate the multiplication loop.

/// 2^26 for the 26-bit limbs.
const BITMASK: u32 = 0x03ffffff;
/// Number is encoded in five 26-bit limbs.
const CARRY: u32 = 26;

/// Poly1305 authenticator takes a 32-byte one-time key and a message and produces a 16-byte tag.
///
/// 64-bit constant time multiplication and addition implementation.
#[derive(Debug)]
pub(crate) struct Poly1305 {
    /// r part of the secret key.
    r: [u32; 5],
    /// s part of the secret key.
    s: [u32; 4],
    /// State used to create tag.
    acc: [u32; 5],
    /// Leftovers between adds.
    leftovers: [u8; 16],
    /// Track relevant leftover bytes.
    leftovers_len: usize,
}

impl Poly1305 {
    /// Initialize authenticator with a 32-byte one-time secret key.
    pub(crate) fn new(key: [u8; 32]) -> Self {
        // Taken from donna. Assigns r to a 26-bit 5-limb number while simultaneously 'clamping' r.
        let r0 =
            u32::from_le_bytes(key[0..4].try_into().expect("infalliable conversion")) & 0x3ffffff;
        let r1 = u32::from_le_bytes(key[3..7].try_into().expect("infalliable conversion")) >> 2
            & 0x03ffff03;
        let r2 = u32::from_le_bytes(key[6..10].try_into().expect("infalliable conversion")) >> 4
            & 0x03ffc0ff;
        let r3 = u32::from_le_bytes(key[9..13].try_into().expect("infalliable conversion")) >> 6
            & 0x03f03fff;
        let r4 = u32::from_le_bytes(key[12..16].try_into().expect("infalliable conversion")) >> 8
            & 0x000fffff;
        let r = [r0, r1, r2, r3, r4];
        let s0 = u32::from_le_bytes(key[16..20].try_into().expect("infalliable conversion"));
        let s1 = u32::from_le_bytes(key[20..24].try_into().expect("infalliable conversion"));
        let s2 = u32::from_le_bytes(key[24..28].try_into().expect("infalliable conversion"));
        let s3 = u32::from_le_bytes(key[28..32].try_into().expect("infalliable conversion"));
        let s = [s0, s1, s2, s3];
        let acc = [0; 5];

        // Initilize leftovers to zero.
        let leftovers = [0u8; 16];
        let leftovers_len = 0;

        Poly1305 {
            r,
            s,
            acc,
            leftovers,
            leftovers_len,
        }
    }

    /// Add message to be authenticated, can be called multiple times before creating tag.
    pub(crate) fn add(&mut self, message: &[u8]) {
        // Deal with previous leftovers if message is long enough.
        let fill = if self.leftovers_len > 0 && (self.leftovers_len + message.len() >= 16) {
            16 - self.leftovers_len
        } else {
            0
        };
        if fill > 0 {
            self.leftovers[self.leftovers_len..].copy_from_slice(&message[0..fill]);

            let msg_slice = prepare_padded_message_slice(&self.leftovers, false);
            for (i, b) in msg_slice.iter().enumerate() {
                self.acc[i] += *b;
            }
            self.r_times_a();
            self.leftovers_len = 0;
        }

        // Remove prefix already processed in leftovers.
        let remaining_message = &message[fill..];

        // Add message to accumulator.
        let mut i = 0;
        while i < remaining_message.len() / 16 {
            let msg_slice =
                prepare_padded_message_slice(&remaining_message[i * 16..(i + 1) * 16], false);
            for (i, b) in msg_slice.iter().enumerate() {
                self.acc[i] += *b;
            }
            self.r_times_a();
            i += 1;
        }

        // Save any leftovers.
        if remaining_message.len() % 16 > 0 {
            let message_index = remaining_message.len() - (remaining_message.len() % 16);
            let new_len = self.leftovers_len + remaining_message.len() % 16;
            self.leftovers[self.leftovers_len..new_len]
                .copy_from_slice(&remaining_message[message_index..]);
            self.leftovers_len = new_len;
        }
    }

    /// Generate authentication tag.
    pub(crate) fn tag(&mut self) -> [u8; 16] {
        // Add any remaining leftovers to accumulator.
        if self.leftovers_len > 0 {
            let msg_slice =
                prepare_padded_message_slice(&self.leftovers[..self.leftovers_len], true);
            for (i, b) in msg_slice.iter().enumerate() {
                self.acc[i] += *b;
            }
            self.r_times_a();
            self.leftovers_len = 0;
        }

        // Carry and mask.
        for i in 1..4 {
            self.acc[i + 1] += self.acc[i] >> CARRY;
        }
        self.acc[0] += (self.acc[4] >> CARRY) * 5;
        self.acc[1] += self.acc[0] >> CARRY;
        for i in 0..self.acc.len() {
            self.acc[i] &= BITMASK;
        }
        // Reduce.
        let mut t = self.acc;
        t[0] += 5;
        t[4] = t[4].wrapping_sub(1 << CARRY);
        for i in 0..3 {
            t[i + 1] += t[i] >> CARRY;
        }
        t[4] = t[4].wrapping_add(t[3] >> CARRY);
        for t in t.iter_mut().take(4) {
            *t &= BITMASK;
        }
        // Convert acc to a 4 item array.
        let mask = (t[4] >> 31).wrapping_sub(1);
        for (i, t) in t.iter().enumerate().take(self.acc.len()) {
            self.acc[i] = t & mask | self.acc[i] & !mask;
        }
        // Voodoo from donna to convert to [u32; 4].
        let a0 = self.acc[0] | self.acc[1] << 26;
        let a1 = self.acc[1] >> 6 | self.acc[2] << 20;
        let a2 = self.acc[2] >> 12 | self.acc[3] << 14;
        let a3 = self.acc[3] >> 18 | self.acc[4] << 8;
        let a = [a0, a1, a2, a3];
        // a + s
        let mut tag: [u64; 4] = [0; 4];
        for i in 0..4 {
            tag[i] = a[i] as u64 + self.s[i] as u64;
        }

        // Carry.
        for i in 0..3 {
            tag[i + 1] += tag[i] >> 32;
        }

        // Return the 16 least significant bytes.
        let mut ret: [u8; 16] = [0; 16];
        for i in 0..tag.len() {
            let bytes = (tag[i] as u32).to_le_bytes();
            ret[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
        }
        ret
    }

    fn r_times_a(&mut self) {
        // Multiply and reduce.
        // While this looks complicated, it is a variation of schoolbook multiplication,
        // described well in an article here: https://loup-vaillant.fr/tutorials/poly1305-design
        let mut t = [0; 5];
        for i in 0..5 {
            for (j, t) in t.iter_mut().enumerate() {
                let modulus: u64 = if i > j { 5 } else { 1 };
                let start = (5 - i) % 5;
                *t += modulus * self.r[i] as u64 * self.acc[(start + j) % 5] as u64;
            }
        }
        // Carry.
        for i in 0..4 {
            t[i + 1] += t[i] >> CARRY;
        }
        // Mask.
        for (i, t) in t.iter().enumerate().take(self.acc.len()) {
            self.acc[i] = *t as u32 & BITMASK;
        }
        // Carry and mask first limb.
        self.acc[0] += (t[4] >> CARRY) as u32 * 5;
        self.acc[1] += self.acc[0] >> CARRY;
        self.acc[0] &= BITMASK;
    }
}

// Encode 16-byte (tag sized), unless is_last flag set to true, piece of message into 5 26-bit limbs.
fn prepare_padded_message_slice(msg: &[u8], is_last: bool) -> [u32; 5] {
    let hi_bit: u32 = if is_last { 0 } else { 1 << 24 };
    let mut fmt_msg = [0u8; 17];
    fmt_msg[..msg.len()].clone_from_slice(msg);
    // Tack on a 1-byte so messages with buncha zeroes at the end don't have the same MAC.
    fmt_msg[msg.len()] = 0x01;
    // Encode number in five 26-bit limbs.
    let m0 = u32::from_le_bytes(fmt_msg[0..4].try_into().expect("Valid subset of 32.")) & BITMASK;
    let m1 =
        u32::from_le_bytes(fmt_msg[3..7].try_into().expect("Valid subset of 32.")) >> 2 & BITMASK;
    let m2 =
        u32::from_le_bytes(fmt_msg[6..10].try_into().expect("Valid subset of 32.")) >> 4 & BITMASK;
    let m3 =
        u32::from_le_bytes(fmt_msg[9..13].try_into().expect("Valid subset of 32.")) >> 6 & BITMASK;
    let m4 =
        u32::from_le_bytes(fmt_msg[12..16].try_into().expect("Valid subset of 32.")) >> 8 | hi_bit;
    [m0, m1, m2, m3, m4]
}

fn _print_acc(num: &[u32; 5]) {
    let a0 = num[0] | num[1] << 26;
    let a1 = num[1] >> 6 | num[2] << 20;
    let a2 = num[2] >> 12 | num[3] << 14;
    let a3 = num[3] >> 18 | num[4] << 8;
    let a = [a0, a1, a2, a3];
    let mut ret: [u8; 16] = [0; 16];
    for i in 0..a.len() {
        let bytes = a[i].to_le_bytes();
        ret[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
    }
    ret.reverse();
}

#[cfg(test)]
#[cfg(feature = "alloc")]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use hex::prelude::*;

    #[test]
    fn test_rfc7539_none_message() {
        let key = Vec::from_hex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b")
            .unwrap();
        let key = key.as_slice().try_into().unwrap();
        let mut poly = Poly1305::new(key);
        let message = b"Cryptographic Forum Research Group";
        poly.add(message);
        let tag = poly.tag();
        assert_eq!(
            "a8061dc1305136c6c22b8baf0c0127a9",
            tag.to_lower_hex_string()
        );
    }
}
