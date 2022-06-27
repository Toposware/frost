// -*- mode: rust; -*-
//
// This file is part of ice-frost.
// Copyright (c) 2020 isis lovecruft
// Copyright (c) 2021-2022 Toposware Inc.
// See LICENSE for licensing information.
//
// Authors:
// - isis agora lovecruft <isis@patternsinthevoid.net>
// - Toposware developers <dev@toposware.com>

//! Precomputation for one-round signing.

use crate::keygen::Error;

#[cfg(feature = "std")]
use std::vec::Vec;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use core::convert::TryInto;

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_TABLE;
use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;

use rand::CryptoRng;
use rand::Rng;

use subtle::Choice;
use subtle::ConstantTimeEq;

use zeroize::Zeroize;

#[derive(Debug, Zeroize)]
#[zeroize(drop)]
pub(crate) struct NoncePair(pub(crate) Scalar, pub(crate) Scalar);

impl NoncePair {
    pub fn new(mut csprng: impl CryptoRng + Rng) -> Self {
        NoncePair(Scalar::random(&mut csprng), Scalar::random(&mut csprng))
    }
}

impl From<NoncePair> for CommitmentShare {
    fn from(other: NoncePair) -> CommitmentShare {
        let x = &RISTRETTO_BASEPOINT_TABLE * &other.0;
        let y = &RISTRETTO_BASEPOINT_TABLE * &other.1;

        CommitmentShare {
            hiding: Commitment {
                nonce: other.0,
                sealed: x,
            },
            binding: Commitment {
                nonce: other.1,
                sealed: y,
            },
        }
    }
}

/// A pair of a nonce and a commitment to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Commitment {
    /// The nonce.
    pub(crate) nonce: Scalar,
    /// The commitment.
    pub(crate) sealed: RistrettoPoint,
}

impl Zeroize for Commitment {
    fn zeroize(&mut self) {
        self.nonce.zeroize();
        self.sealed = RistrettoPoint::identity();
    }
}

impl Drop for Commitment {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Test equality in constant-time.
impl ConstantTimeEq for Commitment {
    fn ct_eq(&self, other: &Commitment) -> Choice {
        self.nonce.ct_eq(&other.nonce) &
            self.sealed.compress().ct_eq(&other.sealed.compress())
    }
}

impl Commitment {
    /// Serialise this commitment to an array of bytes
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut res = [0u8; 64];
        res[0..32].copy_from_slice(&self.nonce.to_bytes());
        res[32..64].copy_from_slice(&self.sealed.compress().to_bytes());

        res
    }

    /// Deserialise this array of bytes to a `Commitment`
    pub fn from_bytes(bytes: &[u8; 64]) -> Result<Commitment, Error> {
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes[0..32]);
        let nonce = Scalar::from_canonical_bytes(array).ok_or(Error::SerialisationError)?;

        array.copy_from_slice(&bytes[32..64]);
        let sealed = CompressedRistretto(array)
            .decompress()
            .ok_or(Error::SerialisationError)?;

        Ok(Commitment { nonce, sealed })
    }
}

/// A precomputed commitment share.
#[derive(Clone, Debug, Eq, PartialEq, Zeroize)]
#[zeroize(drop)]
pub struct CommitmentShare {
    /// The hiding commitment.
    ///
    /// This is \\((d\_{ij}, D\_{ij})\\) in the paper.
    pub(crate) hiding: Commitment,
    /// The binding commitment.
    ///
    /// This is \\((e\_{ij}, E\_{ij})\\) in the paper.
    pub(crate) binding: Commitment,
}

/// Test equality in constant-time.
impl ConstantTimeEq for CommitmentShare {
    fn ct_eq(&self, other: &CommitmentShare) -> Choice {
        self.hiding.ct_eq(&other.hiding) & self.binding.ct_eq(&other.binding)
    }
}

impl CommitmentShare {
    /// Publish the public commitments in this [`CommitmentShare`].
    pub fn publish(&self) -> (RistrettoPoint, RistrettoPoint) {
        (self.hiding.sealed, self.binding.sealed)
    }

    /// Serialise this commitment share to an array of bytes
    pub fn to_bytes(&self) -> [u8; 128] {
        let mut res = [0u8; 128];
        res[0..64].copy_from_slice(&self.hiding.to_bytes());
        res[64..128].copy_from_slice(&self.binding.to_bytes());

        res
    }

    /// Deserialise this array of bytes to a `CommitmentShare`
    pub fn from_bytes(bytes: &[u8; 128]) -> Result<CommitmentShare, Error> {
        let mut array = [0u8; 64];
        array.copy_from_slice(&bytes[0..64]);
        let hiding = Commitment::from_bytes(&array)?;

        array.copy_from_slice(&bytes[64..128]);
        let binding = Commitment::from_bytes(&array)?;

        Ok(CommitmentShare { hiding, binding })
    }
}

/// A secret commitment share list, containing the revealed nonces for the
/// hiding and binding commitments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretCommitmentShareList {
    /// The secret commitment shares.
    pub commitments: Vec<CommitmentShare>,
}

impl SecretCommitmentShareList {
    /// Serialise this secret commitment share list to a Vec of bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut res = Vec::with_capacity(8 + 128 * self.commitments.len());

        let len = self.commitments.len();
        res.extend_from_slice(&TryInto::<u32>::try_into(len).unwrap().to_le_bytes());
        for i in 0..len {
            res.extend_from_slice(&self.commitments[i].to_bytes());
        }

        res
    }

    /// Deserialise this slice of bytes to a `PublicCommitmentShareList`
    pub fn from_bytes(bytes: &[u8]) -> Result<SecretCommitmentShareList, Error> {
        let len = u32::from_le_bytes(
            bytes[0..4]
                .try_into()
                .map_err(|_| Error::SerialisationError)?,
        );
        let mut commitments: Vec<CommitmentShare> = Vec::with_capacity(len as usize);
        let mut index_slice = 4;
        let mut array = [0u8; 128];

        for _ in 0..len {
            array.copy_from_slice(&bytes[index_slice..index_slice + 128]);
            commitments.push(CommitmentShare::from_bytes(&array)?);
            index_slice += 128;
        }
        Ok(SecretCommitmentShareList { commitments })
    }
}

/// A public commitment share list, containing only the hiding and binding
/// commitments, *not* their committed-to nonce values.
///
/// This should be published somewhere before the signing protocol takes place
/// for the other signing participants to obtain.
#[derive(Debug, Eq, PartialEq)]
pub struct PublicCommitmentShareList {
    /// The participant's index.
    pub participant_index: u32,
    /// The published commitments.
    pub commitments: Vec<(RistrettoPoint, RistrettoPoint)>,
}

impl PublicCommitmentShareList {
    /// Serialise this commitment share list to a Vec of bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut res = Vec::with_capacity(8 + 64 * self.commitments.len());
        res.extend_from_slice(&self.participant_index.to_le_bytes());

        let len = self.commitments.len();
        res.extend_from_slice(&TryInto::<u32>::try_into(len).unwrap().to_le_bytes());
        for i in 0..len {
            res.extend_from_slice(&self.commitments[i].0.compress().to_bytes());
            res.extend_from_slice(&self.commitments[i].1.compress().to_bytes());
        }

        res
    }

    /// Deserialise this slice of bytes to a `PublicCommitmentShareList`
    pub fn from_bytes(bytes: &[u8]) -> Result<PublicCommitmentShareList, Error> {
        let participant_index = u32::from_le_bytes(
            bytes[0..4]
                .try_into()
                .map_err(|_| Error::SerialisationError)?,
        );
        let len = u32::from_le_bytes(
            bytes[4..8]
                .try_into()
                .map_err(|_| Error::SerialisationError)?,
        );
        let mut commitments: Vec<(RistrettoPoint, RistrettoPoint)> = Vec::with_capacity(len as usize);
        let mut index_slice = 8;
        let mut array = [0u8; 32];

        for _ in 0..len {
            array.copy_from_slice(&bytes[index_slice..index_slice + 32]);
            let point1 = CompressedRistretto(array).decompress().ok_or(Error::SerialisationError)?;
            array.copy_from_slice(&bytes[index_slice + 32..index_slice + 64]);

            commitments.push((point1, CompressedRistretto(array).decompress().ok_or(Error::SerialisationError)?));
            index_slice += 64;
        }
        Ok(PublicCommitmentShareList {
            participant_index,
            commitments,
        })
    }
}

/// Pre-compute a list of [`CommitmentShare`]s for single-round threshold signing.
///
/// # Inputs
///
/// * `participant_index` is the index of the threshold signing
///   participant who is publishing this share.
/// * `number_of_shares` denotes the number of commitments published at a time.
///
/// # Returns
///
/// A tuple of ([`PublicCommitmentShareList`], [`SecretCommitmentShareList`]).
pub fn generate_commitment_share_lists(
    mut csprng: impl CryptoRng + Rng,
    participant_index: u32,
    number_of_shares: usize,
) -> (PublicCommitmentShareList, SecretCommitmentShareList)
{
    let mut commitments: Vec<CommitmentShare> = Vec::with_capacity(number_of_shares);

    for _ in 0..number_of_shares {
        commitments.push(CommitmentShare::from(NoncePair::new(&mut csprng)));
    }

    let mut published: Vec<(RistrettoPoint, RistrettoPoint)> = Vec::with_capacity(number_of_shares);

    for commitment in commitments.iter() {
        published.push(commitment.publish());
    }

    (PublicCommitmentShareList { participant_index, commitments: published },
     SecretCommitmentShareList { commitments })
}

// XXX TODO This should maybe be a field on SecretKey with some sort of
// regeneration API for generating new share, or warning that there are no
// ununsed shares.
impl SecretCommitmentShareList {
    /// Drop a used [`CommitmentShare`] from our secret commitment share list
    /// and ensure that it is wiped from memory.
    pub fn drop_share(&mut self, share: CommitmentShare) {
        let mut index = -1;

        // This is not constant-time in that the number of commitment shares in
        // the list may be discovered via side channel, as well as the index of
        // share to be deleted, as well as whether or not the share was in the
        // list, but none of this gives any adversary that I can think of any
        // advantage.
        for (i, s) in self.commitments.iter().enumerate() {
            if s.ct_eq(&share).into() {
                index = i as isize;
            }
        }
        if index >= 0 {
            drop(self.commitments.remove(index as usize));
        }
        drop(share);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn test_commitment_serialization() {
        let mut rng = OsRng;

        for _ in 0..100 {
            let nonce = Scalar::random(&mut rng);
            let sealed = &nonce * &curve25519_dalek::constants::RISTRETTO_BASEPOINT_TABLE;

            let hiding = Commitment { nonce, sealed };
            let bytes = hiding.to_bytes();
            assert_eq!(hiding, Commitment::from_bytes(&bytes).unwrap());

            let binding = hiding.clone();
            let commitment_share = CommitmentShare { binding, hiding };
            let bytes = commitment_share.to_bytes();
            assert_eq!(commitment_share, CommitmentShare::from_bytes(&bytes).unwrap());
        }
    }

    #[test]
    fn nonce_pair() {
        let _nonce_pair = NoncePair::new(&mut OsRng);
    }

    #[test]
    fn nonce_pair_into_commitment_share() {
        let _commitment_share: CommitmentShare = NoncePair::new(&mut OsRng).into();
    }

    #[test]
    fn test_serialisation() {
        let mut rng = OsRng;

        for _ in 0..100 {
            let nonce = Scalar::random(&mut rng);
            let sealed = &nonce * &curve25519_dalek::constants::RISTRETTO_BASEPOINT_TABLE;
            let commitment = Commitment { nonce, sealed };

            let bytes = commitment.to_bytes();
            assert!(Commitment::from_bytes(&bytes).is_ok());
            assert_eq!(commitment, Commitment::from_bytes(&bytes).unwrap());
        }

        for _ in 0..100 {
            let nonce = Scalar::random(&mut rng);
            let sealed = &nonce * &curve25519_dalek::constants::RISTRETTO_BASEPOINT_TABLE;
            let binding = Commitment { nonce, sealed };
            let hiding = binding.clone();
            let commitment_share = CommitmentShare { binding, hiding };

            let bytes = commitment_share.to_bytes();
            assert!(CommitmentShare::from_bytes(&bytes).is_ok());
            assert_eq!(commitment_share, CommitmentShare::from_bytes(&bytes).unwrap());
        }

        // invalid encodings
        let bytes = [255u8; 64];
        assert!(Commitment::from_bytes(&bytes).is_err());

        let bytes = [255u8; 128];
        assert!(CommitmentShare::from_bytes(&bytes).is_err());
    }

    #[test]
    fn commitment_share_list_generate() {
        let (public_share_list, secret_share_list) = generate_commitment_share_lists(&mut OsRng, 0, 5);

        assert_eq!(public_share_list.commitments[0].0.compress(),
                   (&secret_share_list.commitments[0].hiding.nonce * &RISTRETTO_BASEPOINT_TABLE).compress());
    }

    #[test]
    fn drop_used_commitment_shares() {
        let (_public_share_list, mut secret_share_list) = generate_commitment_share_lists(&mut OsRng, 3, 8);

        assert!(secret_share_list.commitments.len() == 8);

        let used_share = secret_share_list.commitments[0].clone();

        secret_share_list.drop_share(used_share);

        assert!(secret_share_list.commitments.len() == 7);
    }
}
