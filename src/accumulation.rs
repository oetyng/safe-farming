// Copyright 2020 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use super::{
    AccountAdded, AccountId, AccumulatedClaimed, AccumulationEvent, AmountsAccumulated,
    CurrentAccumulation, WorkCounter,
};
use safe_nd::{Error, Money, Result};
use std::collections::{HashMap, HashSet};

/// The book keeping of rewards.
/// The business rule is that a piece of data
/// is only rewarded once.
pub struct Accumulation {
    idempotency: HashSet<Id>,
    accumulated: HashMap<AccountId, CurrentAccumulation>,
}

pub type Id = Vec<u8>;

impl Accumulation {
    /// ctor
    pub fn new(
        idempotency: HashSet<Id>,
        accumulated: HashMap<AccountId, CurrentAccumulation>,
    ) -> Self {
        Self {
            idempotency,
            accumulated,
        }
    }

    /// -----------------------------------------------------------------
    /// ---------------------- Queries ----------------------------------
    /// -----------------------------------------------------------------

    ///
    pub fn get(&self, account: &AccountId) -> Option<&CurrentAccumulation> {
        self.accumulated.get(account)
    }

    ///
    pub fn get_all(&self) -> &HashMap<AccountId, CurrentAccumulation> {
        &self.accumulated
    }

    /// -----------------------------------------------------------------
    /// ---------------------- Cmds -------------------------------------
    /// -----------------------------------------------------------------

    pub fn add_account(&self, id: AccountId, worked: WorkCounter) -> Result<AccountAdded> {
        if self.accumulated.contains_key(&id) {
            return Err(Error::BalanceExists);
        }
        Ok(AccountAdded { id, worked })
    }

    ///
    pub fn accumulate(
        &self,
        id: Id,
        distribution: HashMap<AccountId, Money>,
    ) -> Result<AmountsAccumulated> {
        if self.idempotency.contains(&id) {
            return Err(Error::DataExists);
        }
        for (id, amount) in &distribution {
            if let Some(existing) = self.accumulated.get(&id) {
                if existing.add(*amount).is_none() {
                    return Err(Error::ExcessiveValue);
                }
            };
        }

        Ok(AmountsAccumulated { id, distribution })
    }

    ///
    pub fn claim(&self, account: AccountId) -> Result<AccumulatedClaimed> {
        let result = self.accumulated.get(&account);
        match result {
            None => Err(Error::NoSuchKey),
            Some(accumulated) => Ok(AccumulatedClaimed {
                account,
                accumulated: accumulated.clone(),
            }),
        }
    }

    /// -----------------------------------------------------------------
    /// ---------------------- Mutation ---------------------------------
    /// -----------------------------------------------------------------

    /// Mutates state.
    pub fn apply(&mut self, event: AccumulationEvent) {
        use AccumulationEvent::*;
        match event {
            AccountAdded(e) => {
                let _ = self.accumulated.insert(
                    e.id,
                    CurrentAccumulation {
                        amount: Money::zero(),
                        worked: e.worked,
                    },
                );
            }
            AmountsAccumulated(e) => {
                for (id, amount) in e.distribution {
                    let existing = match self.accumulated.get(&id) {
                        None => Default::default(),
                        Some(acc) => acc.clone(),
                    };
                    let accumulated = existing.add(amount).unwrap(); // this is OK, since validation shall happen before creating the event
                    let _ = self.idempotency.insert(e.id.clone());
                    let _ = self.accumulated.insert(id, accumulated);
                }
            }
            AccumulatedClaimed(e) => {
                let _ = self.accumulated.remove(&e.account);
            }
        }
    }
}

mod test {
    use super::{Accumulation, AccumulationEvent};
    use safe_nd::{AccountId, Error, Money, PublicKey};
    use threshold_crypto::SecretKey;

    macro_rules! hashmap {
        ($( $key: expr => $val: expr ),*) => {{
             let mut map = ::std::collections::HashMap::new();
             $( let _ = map.insert($key, $val); )*
             map
        }}
    }

    #[test]
    fn when_data_was_not_previously_rewarded_reward_accumulates() {
        // --- Arrange ---
        let mut acc = Accumulation::new(Default::default(), Default::default());
        let account = get_random_pk();
        let data_hash = vec![1, 2, 3];
        let reward = Money::from_nano(10);
        let distribution = hashmap![account => reward];

        // --- Act ---
        // Try accumulate.
        let result = acc.accumulate(data_hash.clone(), distribution.clone());

        // --- Assert ---
        // Confirm valid ..
        match result {
            Err(_) => assert!(false),
            Ok(e) => {
                assert!(e.distribution.len() == 1);
                assert!(e.distribution.contains_key(&account));
                assert_eq!(&reward, e.distribution.get(&account).unwrap());
                acc.apply(AccumulationEvent::AmountsAccumulated(e));
            }
        }
        // .. and successful.
        match acc.get(&account) {
            None => assert!(false),
            Some(accumulated) => assert_eq!(accumulated.amount, reward),
        }
    }

    #[test]
    fn when_data_is_already_rewarded_accumulation_is_rejected() {
        // --- Arrange ---
        let mut acc = Accumulation::new(Default::default(), Default::default());
        let account = get_random_pk();
        let data_hash = vec![1, 2, 3];
        let reward = Money::from_nano(10);
        let distribution = hashmap![account => reward];

        // Accumulate reward.
        let reward = acc
            .accumulate(data_hash.clone(), distribution.clone())
            .unwrap();
        acc.apply(AccumulationEvent::AmountsAccumulated(reward));

        // --- Act ---
        // Try same data hash again ..
        let result = acc.accumulate(data_hash, distribution);

        // --- Assert ---
        // .. confirm not successful.
        match result {
            Ok(_) => assert!(false),
            Err(err) => assert_eq!(err, Error::DataExists),
        }
    }

    #[test]
    fn when_account_has_reward_it_can_claim() {
        // --- Arrange ---
        let mut acc = Accumulation::new(Default::default(), Default::default());
        let account = get_random_pk();
        let data_hash = vec![1, 2, 3];
        let reward = Money::from_nano(10);
        let distribution = hashmap![account => reward];
        let accumulation = acc
            .accumulate(data_hash.clone(), distribution.clone())
            .unwrap();
        acc.apply(AccumulationEvent::AmountsAccumulated(accumulation));

        // --- Act + Assert ---
        // Try claim, confirm account and amount is correct.
        let result = acc.claim(account);
        match result {
            Err(_) => assert!(false),
            Ok(e) => {
                assert!(e.account == account);
                assert!(e.accumulated.amount == reward);
                acc.apply(AccumulationEvent::AccumulatedClaimed(e));
            }
        }
    }

    #[test]
    fn when_reward_was_claimed_it_can_not_be_claimed_again() {
        // --- Arrange ---
        let mut acc = Accumulation::new(Default::default(), Default::default());
        let account = get_random_pk();
        let data_hash = vec![1, 2, 3];
        let reward = Money::from_nano(10);
        let distribution = hashmap![account => reward];

        let accumulation = acc.accumulate(data_hash, distribution).unwrap();
        acc.apply(AccumulationEvent::AmountsAccumulated(accumulation));

        // Claim the account reward.
        let claim = acc.claim(account).unwrap();
        acc.apply(AccumulationEvent::AccumulatedClaimed(claim));

        // --- Act ---
        // Try claim the account reward again ..
        let result = acc.claim(account);

        // --- Assert ---
        // .. confirm not successful.
        match result {
            Ok(_) => assert!(false),
            Err(err) => assert_eq!(err, Error::NoSuchKey),
        }
    }

    #[test]
    fn when_account_has_no_reward_it_can_not_claim() {
        // --- Arrange ---
        let acc = Accumulation::new(Default::default(), Default::default());
        let account = get_random_pk();

        // --- Act + Assert ---
        // Try claim the account reward again, confirm not successful.
        let result = acc.claim(account);
        match result {
            Ok(_) => assert!(false),
            Err(err) => assert_eq!(err, Error::NoSuchKey),
        }
    }

    #[test]
    fn when_reward_was_claimed_get_returns_none() {
        // --- Arrange ---
        let mut acc = Accumulation::new(Default::default(), Default::default());
        let account = get_random_pk();
        let data_hash = vec![1, 2, 3];
        let reward = Money::from_nano(10);
        let distribution = hashmap![account => reward];
        let accumulation = acc.accumulate(data_hash, distribution).unwrap();
        acc.apply(AccumulationEvent::AmountsAccumulated(accumulation));
        let claim = acc.claim(account).unwrap();
        acc.apply(AccumulationEvent::AccumulatedClaimed(claim));

        // --- Act ---
        // Try get the account reward.
        let result = acc.get(&account);

        // --- Assert ---
        assert!(result.is_none());
    }

    fn get_random_pk() -> PublicKey {
        PublicKey::from(SecretKey::random().public_key())
    }
}
