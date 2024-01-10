// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Cumulus is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Cumulus is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit = "256"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

pub mod bridge_to_ethereum_config;
pub mod bridge_to_kusama_config;
mod weights;
pub mod xcm_config;

use cumulus_pallet_parachain_system::RelayNumberStrictlyIncreases;
use snowbridge_beacon_primitives::{Fork, ForkVersions};
use snowbridge_core::{
	gwei, meth, outbound::Message, AgentId, AllowSiblingsOnly, PricingParameters, Rewards,
};
use snowbridge_router_primitives::inbound::MessageToXcm;
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata, H160};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{AccountIdLookup, BlakeTwo256, Block as BlockT, Keccak256},
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult,
};

use sp_std::prelude::*;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

#[cfg(not(feature = "runtime-benchmarks"))]
use bridge_hub_common::BridgeHubMessageRouter;
use bridge_hub_common::{
	message_queue::{NarrowOriginToSibling, ParaIdToSibling},
	AggregateMessageOrigin,
};
use cumulus_primitives_core::ParaId;
use frame_support::{
	construct_runtime,
	dispatch::DispatchClass,
	genesis_builder_helper::{build_config, create_default_config},
	parameter_types,
	traits::{ConstBool, ConstU32, ConstU64, ConstU8, EitherOfDiverse, Everything},
	weights::{ConstantMultiplier, Weight},
	PalletId,
};
use frame_system::{
	limits::{BlockLength, BlockWeights},
	EnsureRoot,
};
use pallet_xcm::{EnsureXcm, IsVoiceOfBody};
pub use sp_consensus_aura::sr25519::AuthorityId as AuraId;
pub use sp_runtime::{MultiAddress, Perbill, Permill};
use xcm_config::{
	FellowshipLocation, GovernanceLocation, XcmConfig, TreasuryAccount,
	XcmOriginToTransactDispatchOrigin,
};

#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

// Polkadot imports
use polkadot_runtime_common::{BlockHashCount, SlowAdjustingFeeUpdate};
use polkadot_runtime_constants::system_parachain::{ASSET_HUB_ID, BRIDGE_HUB_ID};

use weights::{BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight};

use parachains_common::{
	impls::DealWithFees, AccountId, Balance, BlockNumber, Hash, Header, Nonce, Signature,
};
use system_parachains_constants::{
	polkadot::{consensus::*, currency::*, fee::WeightToFee},
	AVERAGE_ON_INITIALIZE_RATIO, HOURS, MAXIMUM_BLOCK_WEIGHT, NORMAL_DISPATCH_RATIO, SLOT_DURATION,
};
// XCM Imports
use xcm::prelude::*;
use xcm_executor::XcmExecutor;

#[cfg(feature = "runtime-benchmarks")]
use crate::xcm_config::benchmark_helpers::DoNothingRouter;
#[cfg(feature = "runtime-benchmarks")]
use snowbridge_beacon_primitives::CompactExecutionHeader;
#[cfg(feature = "runtime-benchmarks")]
use snowbridge_core::RingBufferMap;
#[cfg(feature = "runtime-benchmarks")]
pub use snowbridge_ethereum_beacon_client::ExecutionHeaderBuffer;
#[cfg(feature = "runtime-benchmarks")]
use snowbridge_inbound_queue::BenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
use sp_core::H256;

/// The address format for describing accounts.
pub type Address = MultiAddress<AccountId, ()>;

/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;

/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;

/// The SignedExtension to the basic transaction logic.
pub type SignedExtra = (
	frame_system::CheckNonZeroSender<Runtime>,
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckEra<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
	BridgeRejectObsoleteHeadersAndMessages,
	bridge_to_kusama_config::RefundBridgeHubKusamaMessages,
);

bridge_runtime_common::generate_bridge_reject_obsolete_headers_and_messages! {
	RuntimeCall, AccountId,
	// Grandpa
	BridgeKusamaGrandpa,
	// Parachains
	BridgeKusamaParachains,
	// Messages
	BridgeKusamaMessages
}

/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;

/// Migrations to apply on runtime upgrade.
pub type Migrations = ();

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
	Migrations,
>;

impl_opaque_keys! {
	pub struct SessionKeys {
		pub aura: Aura,
	}
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("bridge-hub-polkadot"),
	impl_name: create_runtime_str!("bridge-hub-polkadot"),
	authoring_version: 1,
	spec_version: 1_001_000,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 3,
	state_version: 1,
};

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
	pub RuntimeBlockLength: BlockLength =
		BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
		.base_block(BlockExecutionWeight::get())
		.for_class(DispatchClass::all(), |weights| {
			weights.base_extrinsic = ExtrinsicBaseWeight::get();
		})
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
			// Operational transactions have some extra reserved space, so that they
			// are included even if block reached `MAXIMUM_BLOCK_WEIGHT`.
			weights.reserved = Some(
				MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT
			);
		})
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic();
	pub const SS58Prefix: u8 = 0;
}

// Configure FRAME pallets to include in runtime.

impl frame_system::Config for Runtime {
	/// The identifier used to distinguish between accounts.
	type AccountId = AccountId;
	/// The aggregated dispatch type that is available for extrinsics.
	type RuntimeCall = RuntimeCall;
	/// The lookup mechanism to get account ID from whatever is passed in dispatchers.
	type Lookup = AccountIdLookup<AccountId, ()>;
	/// The index type for storing how many extrinsics an account has signed.
	type Nonce = Nonce;
	/// The type for hashing blocks and tries.
	type Hash = Hash;
	/// The hashing algorithm used.
	type Hashing = BlakeTwo256;
	/// The block type.
	type Block = Block;
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	/// The ubiquitous origin type.
	type RuntimeOrigin = RuntimeOrigin;
	/// Maximum number of block number to block hash mappings to keep (oldest pruned first).
	type BlockHashCount = BlockHashCount;
	/// Runtime version.
	type Version = Version;
	/// Converts a module to an index of this module in the runtime.
	type PalletInfo = PalletInfo;
	/// The data to be stored in an account.
	type AccountData = pallet_balances::AccountData<Balance>;
	/// What to do if a new account is created.
	type OnNewAccount = ();
	/// What to do if an account is fully reaped from the system.
	type OnKilledAccount = ();
	/// The weight of database operations that the runtime can invoke.
	type DbWeight = RocksDbWeight;
	/// The basic call filter to use in dispatchable.
	type BaseCallFilter = Everything;
	/// Weight information for the extrinsics of this pallet.
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
	/// Block & extrinsics weights: base values and limits.
	type BlockWeights = RuntimeBlockWeights;
	/// The maximum length of a block (in bytes).
	type BlockLength = RuntimeBlockLength;
	type SS58Prefix = SS58Prefix;
	/// The action to take on a Runtime Upgrade
	type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
	type MaxConsumers = ConstU32<16>;
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
	type WeightInfo = weights::pallet_timestamp::WeightInfo<Runtime>;
}

impl pallet_authorship::Config for Runtime {
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Aura>;
	type EventHandler = (CollatorSelection,);
}

parameter_types! {
	pub const ExistentialDeposit: Balance = SYSTEM_PARA_EXISTENTIAL_DEPOSIT;
}

impl pallet_balances::Config for Runtime {
	/// The type for recording an account's balance.
	type Balance = Balance;
	type DustRemoval = ();
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
	type MaxLocks = ConstU32<50>;
	type MaxReserves = ConstU32<50>;
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<0>;
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	/// Relay Chain `TransactionByteFee` / 10
	pub const TransactionByteFee: Balance = MILLICENTS;
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnChargeTransaction =
		pallet_transaction_payment::CurrencyAdapter<Balances, DealWithFees<Runtime>>;
	type OperationalFeeMultiplier = ConstU8<5>;
	type WeightToFee = WeightToFee;
	type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
	type FeeMultiplierUpdate = SlowAdjustingFeeUpdate<Self>;
}

parameter_types! {
	pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
	pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
}

impl cumulus_pallet_parachain_system::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnSystemEvent = ();
	type SelfParaId = parachain_info::Pallet<Runtime>;
	type OutboundXcmpMessageSource = XcmpQueue;
	type DmpMessageHandler = DmpQueue;
	type ReservedDmpWeight = ReservedDmpWeight;
	type XcmpMessageHandler = XcmpQueue;
	type ReservedXcmpWeight = ReservedXcmpWeight;
	type CheckAssociatedRelayNumber = RelayNumberStrictlyIncreases;
	type ConsensusHook = cumulus_pallet_aura_ext::FixedVelocityConsensusHook<
		Runtime,
		RELAY_CHAIN_SLOT_DURATION_MILLIS,
		BLOCK_PROCESSING_VELOCITY,
		UNINCLUDED_SEGMENT_CAPACITY,
	>;
}

impl parachain_info::Config for Runtime {}

impl cumulus_pallet_aura_ext::Config for Runtime {}

// todo add message queue pallet config impl pallet_message_queue::Config for Runtime {

parameter_types! {
	// Fellows pluralistic body.
	pub const FellowsBodyId: BodyId = BodyId::Technical;
	/// The asset ID for the asset that we use to pay for message delivery fees.
	pub FeeAssetId: AssetId = Concrete(xcm_config::DotRelayLocation::get());
	/// The base fee for the message delivery fees.
	pub const ToSiblingBaseDeliveryFee: u128 = CENTS.saturating_mul(3);
	pub const ToParentBaseDeliveryFee: u128 = CENTS.saturating_mul(3);
}

/// Privileged origin that represents Root or Fellows.
pub type RootOrFellows = EitherOfDiverse<
	EnsureRoot<AccountId>,
	EnsureXcm<IsVoiceOfBody<FellowshipLocation, FellowsBodyId>>,
>;

pub type PriceForSiblingParachainDelivery = polkadot_runtime_common::xcm_sender::ExponentialPrice<
	FeeAssetId,
	ToSiblingBaseDeliveryFee,
	TransactionByteFee,
	XcmpQueue,
>;
pub type PriceForParentDelivery = polkadot_runtime_common::xcm_sender::ExponentialPrice<
	FeeAssetId,
	ToParentBaseDeliveryFee,
	TransactionByteFee,
	ParachainSystem,
>;

impl cumulus_pallet_xcmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type ChannelInfo = ParachainSystem;
	type VersionWrapper = PolkadotXcm;
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
	type ControllerOrigin = RootOrFellows;
	type ControllerOriginConverter = XcmOriginToTransactDispatchOrigin;
	type WeightInfo = weights::cumulus_pallet_xcmp_queue::WeightInfo<Runtime>;
	type PriceForSiblingDelivery = PriceForSiblingParachainDelivery;
}

impl cumulus_pallet_dmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
}

pub const PERIOD: u32 = 6 * HOURS;
pub const OFFSET: u32 = 0;

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	// we don't have stash and controller, thus we don't need the convert as well.
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ShouldEndSession = pallet_session::PeriodicSessions<ConstU32<PERIOD>, ConstU32<OFFSET>>;
	type NextSessionRotation = pallet_session::PeriodicSessions<ConstU32<PERIOD>, ConstU32<OFFSET>>;
	type SessionManager = CollatorSelection;
	// Essentially just Aura, but let's be pedantic.
	type SessionHandler = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
}

impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type DisabledValidators = ();
	type MaxAuthorities = ConstU32<100_000>;
	type AllowMultipleBlocksPerSlot = ConstBool<false>;
	#[cfg(feature = "experimental")]
	type SlotDuration = pallet_aura::MinimumPeriodTimesTwo<Self>;
}

parameter_types! {
	pub const PotId: PalletId = PalletId(*b"PotStake");
	pub const SessionLength: BlockNumber = 6 * HOURS;
	// StakingAdmin pluralistic body.
	pub const StakingAdminBodyId: BodyId = BodyId::Defense;
}

/// We allow root, the StakingAdmin to execute privileged collator selection operations.
pub type CollatorSelectionUpdateOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	EnsureXcm<IsVoiceOfBody<GovernanceLocation, StakingAdminBodyId>>,
>;

impl pallet_collator_selection::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type UpdateOrigin = CollatorSelectionUpdateOrigin;
	type PotId = PotId;
	type MaxCandidates = ConstU32<100>;
	type MinEligibleCollators = ConstU32<4>;
	type MaxInvulnerables = ConstU32<20>;
	// should be a multiple of session or things will get inconsistent
	type KickThreshold = ConstU32<PERIOD>;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ValidatorRegistration = Session;
	type WeightInfo = weights::pallet_collator_selection::WeightInfo<Runtime>;
}

parameter_types! {
	// One storage item; key size is 32; value is size 4+4+16+32 bytes = 56 bytes.
	pub const DepositBase: Balance = system_para_deposit(1, 88);
	// Additional storage item size of 32 bytes.
	pub const DepositFactor: Balance = system_para_deposit(0, 32);
}

impl pallet_multisig::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type DepositBase = DepositBase;
	type DepositFactor = DepositFactor;
	type MaxSignatories = ConstU32<100>;
	type WeightInfo = weights::pallet_multisig::WeightInfo<Runtime>;
}

impl pallet_utility::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type PalletsOrigin = OriginCaller;
	type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

// Ethereum Bridge

#[cfg(not(feature = "runtime-benchmarks"))]
parameter_types! {
	pub storage EthereumGatewayAddress: H160 = H160::zero();
}

#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
	pub storage EthereumGatewayAddress: H160 = H160(hex_literal::hex!("EDa338E4dC46038493b885327842fD3E301CaB39"));
}

parameter_types! {
	pub const CreateAssetCall: [u8;2] = [53, 0];
	pub const CreateAssetDeposit: u128 = (UNITS / 10) + EXISTENTIAL_DEPOSIT;
	pub const InboundQueuePalletInstance: u8 = snowbridge_kusama_common::INBOUND_QUEUE_MESSAGES_PALLET_INDEX;
	pub Parameters: PricingParameters<u128> = PricingParameters {
		exchange_rate: FixedU128::from_rational(1, 400),
		fee_per_gas: gwei(20),
		rewards: Rewards { local: 1 * UNITS, remote: meth(1) }
	};
}

#[cfg(feature = "runtime-benchmarks")]
impl<T: snowbridge_ethereum_beacon_client::Config> BenchmarkHelper<T> for Runtime {
	fn initialize_storage(block_hash: H256, header: CompactExecutionHeader) {
		<ExecutionHeaderBuffer<T>>::insert(block_hash, header);
	}
}

impl snowbridge_inbound_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Verifier = snowbridge_ethereum_beacon_client::Pallet<Runtime>;
	type Token = Balances;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type XcmSender = XcmRouter;
	#[cfg(feature = "runtime-benchmarks")]
	type XcmSender = DoNothingRouter;
	type ChannelLookup = EthereumSystem;
	type GatewayAddress = EthereumGatewayAddress;
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = Runtime;
	type MessageConverter = MessageToXcm<
		CreateAssetCall,
		CreateAssetDeposit,
		InboundQueuePalletInstance,
		AccountId,
		Balance,
	>;
	type WeightToFee = WeightToFee;
	type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
	type MaxMessageSize = ConstU32<2048>;
	type WeightInfo = weights::snowbridge_inbound_queue::WeightInfo<Runtime>;
	type PricingParameters = EthereumSystem;
}

impl snowbridge_outbound_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Hashing = Keccak256;
	type MessageQueue = MessageQueue;
	type Decimals = ConstU8<12>;
	type MaxMessagePayloadSize = ConstU32<2048>;
	type MaxMessagesPerBlock = ConstU32<32>;
	type GasMeter = snowbridge_core::outbound::ConstantGasMeter;
	type Balance = Balance;
	type WeightToFee = WeightToFee;
	type WeightInfo = weights::snowbridge_outbound_queue::WeightInfo<Runtime>;
	type PricingParameters = EthereumSystem;
	type Channels = EthereumSystem;
}

#[cfg(not(feature = "beacon-spec-mainnet"))]
parameter_types! {
	pub const ChainForkVersions: ForkVersions = ForkVersions {
		genesis: Fork {
			version: [0, 0, 0, 1], // 0x00000001
			epoch: 0,
		},
		altair: Fork {
			version: [1, 0, 0, 1], // 0x01000001
			epoch: 0,
		},
		bellatrix: Fork {
			version: [2, 0, 0, 1], // 0x02000001
			epoch: 0,
		},
		capella: Fork {
			version: [3, 0, 0, 1], // 0x03000001
			epoch: 0,
		},
	};
	pub const MaxExecutionHeadersToKeep:u32 = 1000;
}

#[cfg(feature = "beacon-spec-mainnet")]
parameter_types! {
	pub const ChainForkVersions: ForkVersions = ForkVersions {
		genesis: Fork {
			version: [0, 0, 0, 0], // 0x00000000
			epoch: 0,
		},
		altair: Fork {
			version: [1, 0, 0, 0], // 0x01000000
			epoch: 74240,
		},
		bellatrix: Fork {
			version: [2, 0, 0, 0], // 0x02000000
			epoch: 144896,
		},
		capella: Fork {
			version: [3, 0, 0, 0], // 0x03000000
			epoch: 194048,
		},
	};
	pub const MaxExecutionHeadersToKeep:u32 = 8192 * 2;
}

impl snowbridge_ethereum_beacon_client::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ForkVersions = ChainForkVersions;
	type MaxExecutionHeadersToKeep = MaxExecutionHeadersToKeep;
	type WeightInfo = weights::snowbridge_ethereum_beacon_client::WeightInfo<Runtime>;
}

#[cfg(feature = "runtime-benchmarks")]
impl snowbridge_system::BenchmarkHelper<RuntimeOrigin> for () {
	fn make_xcm_origin(location: xcm::latest::MultiLocation) -> RuntimeOrigin {
		RuntimeOrigin::from(pallet_xcm::Origin::Xcm(location))
	}
}

impl snowbridge_system::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OutboundQueue = EthereumOutboundQueue;
	type SiblingOrigin = EnsureXcm<AllowSiblingsOnly>;
	type AgentIdOf = snowbridge_core::AgentIdOf;
	type TreasuryAccount = TreasuryAccount;
	type Token = Balances;
	type WeightInfo = weights::snowbridge_system::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = ();
	type DefaultPricingParameters = Parameters;
	type InboundDeliveryCost = EthereumInboundQueue;
}

// Create the runtime by composing the FRAME pallets that were previously configured.
construct_runtime!(
	pub enum Runtime
	{
		// System support stuff.
		System: frame_system = 0,
		ParachainSystem: cumulus_pallet_parachain_system = 1,
		Timestamp: pallet_timestamp = 2,
		ParachainInfo: parachain_info = 3,

		// Monetary stuff.
		Balances: pallet_balances = 10,
		TransactionPayment: pallet_transaction_payment = 11,

		// Collator support. The order of these 4 are important and shall not change.
		Authorship: pallet_authorship = 20,
		CollatorSelection: pallet_collator_selection = 21,
		Session: pallet_session = 22,
		Aura: pallet_aura = 23,
		AuraExt: cumulus_pallet_aura_ext = 24,

		// XCM helpers.
		XcmpQueue: cumulus_pallet_xcmp_queue = 30,
		PolkadotXcm: pallet_xcm = 31,
		CumulusXcm: cumulus_pallet_xcm = 32,
		DmpQueue: cumulus_pallet_dmp_queue = 33,

		// Handy utilities.
		Utility: pallet_utility = 40,
		Multisig: pallet_multisig = 41,

		// Pallets that may be used by all bridges.
		BridgeRelayers: pallet_bridge_relayers = 50,

		// Kusama bridge pallets.
		BridgeKusamaGrandpa: pallet_bridge_grandpa::<Instance1> = 51,
		BridgeKusamaParachains: pallet_bridge_parachains::<Instance1> = 52,
		BridgeKusamaMessages: pallet_bridge_messages::<Instance1> = 53,

		EthereumInboundQueue: snowbridge_inbound_queue = 80,
		EthereumOutboundQueue: snowbridge_outbound_queue = 81,
		EthereumBeaconClient: snowbridge_ethereum_beacon_client = 82,
		EthereumSystem: snowbridge_system = 83,
	}
);

#[cfg(feature = "runtime-benchmarks")]
mod benches {
	frame_benchmarking::define_benchmarks!(
		[frame_system, SystemBench::<Runtime>]
		[pallet_balances, Balances]
		[pallet_multisig, Multisig]
		[pallet_session, SessionBench::<Runtime>]
		[pallet_utility, Utility]
		[pallet_timestamp, Timestamp]
		[pallet_collator_selection, CollatorSelection]
		[cumulus_pallet_xcmp_queue, XcmpQueue]
		// XCM
		[pallet_xcm, PalletXcmExtrinsiscsBenchmark::<Runtime>]
		// NOTE: Make sure you point to the individual modules below.
		[pallet_xcm_benchmarks::fungible, XcmBalances]
		[pallet_xcm_benchmarks::generic, XcmGeneric]
		// Shared bridge pallets
		[pallet_bridge_relayers, BridgeRelayersBench::<Runtime>]
		// Polkadot bridge pallets.
		[pallet_bridge_grandpa, KusamaFinality]
		[pallet_bridge_parachains, KusamaParachains]
		[pallet_bridge_messages, KusamaMessages]
		// Ethereum Bridge
		[snowbridge_inbound_queue, EthereumInboundQueue]
		[snowbridge_outbound_queue, EthereumOutboundQueue]
		[snowbridge_system, EthereumSystem]
		[snowbridge_ethereum_beacon_client, EthereumBeaconClient]
	);
}

impl_runtime_apis! {
	impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> sp_consensus_aura::SlotDuration {
			sp_consensus_aura::SlotDuration::from_millis(Aura::slot_duration())
		}

		fn authorities() -> Vec<AuraId> {
			Aura::authorities().into_inner()
		}
	}

	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialize_block(header: &<Block as BlockT>::Header) {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}

		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> sp_std::vec::Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: Block,
			data: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
		fn query_info(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
		for Runtime
	{
		fn query_call_info(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_call_info(call, len)
		}
		fn query_call_fee_details(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_call_fee_details(call, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
		fn collect_collation_info(header: &<Block as BlockT>::Header) -> cumulus_primitives_core::CollationInfo {
			ParachainSystem::collect_collation_info(header)
		}
	}

	impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
		fn create_default_config() -> Vec<u8> {
			create_default_config::<RuntimeGenesisConfig>()
		}

		fn build_config(config: Vec<u8>) -> sp_genesis_builder::Result {
			build_config::<RuntimeGenesisConfig>(config)
		}
	}

	impl bp_kusama::KusamaFinalityApi<Block> for Runtime {
		fn best_finalized() -> Option<bp_runtime::HeaderId<bp_kusama::Hash, bp_kusama::BlockNumber>> {
			BridgeKusamaGrandpa::best_finalized()
		}

		fn synced_headers_grandpa_info(
		) -> Vec<bp_header_chain::StoredHeaderGrandpaInfo<bp_kusama::Header>> {
			BridgeKusamaGrandpa::synced_headers_grandpa_info()
		}
	}

	impl bp_bridge_hub_kusama::BridgeHubKusamaFinalityApi<Block> for Runtime {
		fn best_finalized() -> Option<bp_runtime::HeaderId<bp_bridge_hub_kusama::Hash, bp_bridge_hub_kusama::BlockNumber>> {
			BridgeKusamaParachains::best_parachain_head_id::<
				bp_bridge_hub_kusama::BridgeHubKusama
			>().unwrap_or(None)
		}
	}

	impl bp_bridge_hub_kusama::FromBridgeHubKusamaInboundLaneApi<Block> for Runtime {
		fn message_details(
			lane: bp_messages::LaneId,
			messages: Vec<(bp_messages::MessagePayload, bp_messages::OutboundMessageDetails)>,
		) -> Vec<bp_messages::InboundMessageDetails> {
			bridge_runtime_common::messages_api::inbound_message_details::<
				Runtime,
				bridge_to_kusama_config::WithBridgeHubKusamaMessagesInstance,
			>(lane, messages)
		}
	}

	impl bp_bridge_hub_kusama::ToBridgeHubKusamaOutboundLaneApi<Block> for Runtime {
		fn message_details(
			lane: bp_messages::LaneId,
			begin: bp_messages::MessageNonce,
			end: bp_messages::MessageNonce,
		) -> Vec<bp_messages::OutboundMessageDetails> {
			bridge_runtime_common::messages_api::outbound_message_details::<
				Runtime,
				bridge_to_kusama_config::WithBridgeHubKusamaMessagesInstance,
			>(lane, begin, end)
		}
	}

	impl snowbridge_outbound_queue_runtime_api::OutboundQueueApi<Block, Balance> for Runtime {
		fn prove_message(leaf_index: u64) -> Option<snowbridge_outbound_queue::MerkleProof> {
			snowbridge_outbound_queue::api::prove_message::<Runtime>(leaf_index)
		}

		fn calculate_fee(message: Message) -> Option<Balance> {
			snowbridge_outbound_queue::api::calculate_fee::<Runtime>(message)
		}
	}

	impl snowbridge_system_runtime_api::ControlApi<Block> for Runtime {
		fn agent_id(location: VersionedMultiLocation) -> Option<AgentId> {
			snowbridge_system::api::agent_id::<Runtime>(location)
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, RuntimeBlockWeights::get().max_block)
		}

		fn execute_block(
			block: Block,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect,
		) -> Weight {
			// NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
			// have a backtrace here.
			Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{Benchmarking, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;
			use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsiscsBenchmark;
			use frame_system_benchmarking::Pallet as SystemBench;
			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;

			// This is defined once again in dispatch_benchmark, because list_benchmarks!
			// and add_benchmarks! are macros exported by define_benchmarks! macros and those types
			// are referenced in that call.
			type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet::<Runtime>;
			type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet::<Runtime>;

			use pallet_bridge_relayers::benchmarking::Pallet as BridgeRelayersBench;
			type KusamaFinality = BridgeKusamaGrandpa;
			type KusamaParachains = pallet_bridge_parachains::benchmarking::Pallet::<Runtime, bridge_to_kusama_config::BridgeParachainKusamaInstance>;
			type KusamaMessages = pallet_bridge_messages::benchmarking::Pallet::<Runtime, bridge_to_kusama_config::WithBridgeHubKusamaMessagesInstance>;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();
			(list, storage_info)
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, sp_runtime::RuntimeString> {
			use frame_benchmarking::{Benchmarking, BenchmarkBatch, BenchmarkError};
			use sp_storage::TrackedStorageKey;

			use frame_system_benchmarking::Pallet as SystemBench;
			impl frame_system_benchmarking::Config for Runtime {
				fn setup_set_code_requirements(code: &sp_std::vec::Vec<u8>) -> Result<(), BenchmarkError> {
					ParachainSystem::initialize_for_set_code_benchmark(code.len() as u32);
					Ok(())
				}

				fn verify_set_code() {
					System::assert_last_event(cumulus_pallet_parachain_system::Event::<Runtime>::ValidationFunctionStored.into());
				}
			}

			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
			impl cumulus_pallet_session_benchmarking::Config for Runtime {}

			use xcm::latest::prelude::*;
			use xcm_config::DotRelayLocation;

			use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsiscsBenchmark;
			impl pallet_xcm::benchmarking::Config for Runtime {
				fn reachable_dest() -> Option<MultiLocation> {
					Some(Parent.into())
				}

				fn teleportable_asset_and_dest() -> Option<(MultiAsset, MultiLocation)> {
					// Relay/native token can be teleported between BH and Relay.
					Some((
						MultiAsset {
							fun: Fungible(EXISTENTIAL_DEPOSIT),
							id: Concrete(Parent.into())
						},
						Parent.into(),
					))
				}

				fn reserve_transferable_asset_and_dest() -> Option<(MultiAsset, MultiLocation)> {
					// Reserve transfers are disabled on BH.
					None
				}
			}

			parameter_types! {
				pub ExistentialDepositMultiAsset: Option<MultiAsset> = Some((
					DotRelayLocation::get(),
					ExistentialDeposit::get()
				).into());
			}

			impl pallet_xcm_benchmarks::Config for Runtime {
				type XcmConfig = xcm_config::XcmConfig;
				type AccountIdConverter = xcm_config::LocationToAccountId;
				type DeliveryHelper = cumulus_primitives_utility::ToParentDeliveryHelper<
					xcm_config::XcmConfig,
					ExistentialDepositMultiAsset,
					PriceForParentDelivery,
				>;
				fn valid_destination() -> Result<MultiLocation, BenchmarkError> {
					Ok(DotRelayLocation::get())
				}
				fn worst_case_holding(_depositable_count: u32) -> MultiAssets {
					// just concrete assets according to relay chain.
					let assets: Vec<MultiAsset> = vec![
						MultiAsset {
							id: Concrete(DotRelayLocation::get()),
							fun: Fungible(1_000_000 * UNITS),
						}
					];
					assets.into()
				}
			}

			parameter_types! {
				pub const TrustedTeleporter: Option<(MultiLocation, MultiAsset)> = Some((
					DotRelayLocation::get(),
					MultiAsset { fun: Fungible(UNITS), id: Concrete(DotRelayLocation::get()) },
				));
				pub const CheckedAccount: Option<(AccountId, xcm_builder::MintLocation)> = None;
				pub const TrustedReserve: Option<(MultiLocation, MultiAsset)> = None;
			}

			impl pallet_xcm_benchmarks::fungible::Config for Runtime {
				type TransactAsset = Balances;

				type CheckedAccount = CheckedAccount;
				type TrustedTeleporter = TrustedTeleporter;
				type TrustedReserve = TrustedReserve;

				fn get_multi_asset() -> MultiAsset {
					MultiAsset {
						id: Concrete(DotRelayLocation::get()),
						fun: Fungible(UNITS),
					}
				}
			}

			impl pallet_xcm_benchmarks::generic::Config for Runtime {
				type TransactAsset = Balances;
				type RuntimeCall = RuntimeCall;

				fn worst_case_response() -> (u64, Response) {
					(0u64, Response::Version(Default::default()))
				}

				fn worst_case_asset_exchange() -> Result<(MultiAssets, MultiAssets), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn universal_alias() -> Result<(MultiLocation, Junction), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn transact_origin_and_runtime_call() -> Result<(MultiLocation, RuntimeCall), BenchmarkError> {
					Ok((DotRelayLocation::get(), frame_system::Call::remark_with_event { remark: vec![] }.into()))
				}

				fn subscribe_origin() -> Result<MultiLocation, BenchmarkError> {
					Ok(DotRelayLocation::get())
				}

				fn claimable_asset() -> Result<(MultiLocation, MultiLocation, MultiAssets), BenchmarkError> {
					let origin = DotRelayLocation::get();
					let assets: MultiAssets = (Concrete(DotRelayLocation::get()), 1_000 * UNITS).into();
					let ticket = MultiLocation { parents: 0, interior: Here };
					Ok((origin, ticket, assets))
				}

				fn fee_asset() -> Result<MultiAsset, BenchmarkError> {
					Ok(MultiAsset {
						id: Concrete(DotRelayLocation::get()),
						fun: Fungible(1_000_000 * UNITS),
					})
				}

				fn unlockable_asset() -> Result<(MultiLocation, MultiLocation, MultiAsset), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn export_message_origin_and_destination(
				) -> Result<(MultiLocation, NetworkId, InteriorMultiLocation), BenchmarkError> {
					Ok((DotRelayLocation::get(), NetworkId::Kusama, X1(Parachain(1000))))
				}

				fn alias_origin() -> Result<(MultiLocation, MultiLocation), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}
			}

			type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet::<Runtime>;
			type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet::<Runtime>;

			type KusamaFinality = BridgeKusamaGrandpa;
			type KusamaParachains = pallet_bridge_parachains::benchmarking::Pallet::<Runtime, bridge_to_kusama_config::BridgeParachainKusamaInstance>;
			type KusamaMessages = pallet_bridge_messages::benchmarking::Pallet::<Runtime, bridge_to_kusama_config::WithBridgeHubKusamaMessagesInstance>;

			use pallet_bridge_relayers::benchmarking::{
				Pallet as BridgeRelayersBench,
				Config as BridgeRelayersConfig,
			};

			impl BridgeRelayersConfig for Runtime {
				fn prepare_rewards_account(
					account_params: bp_relayers::RewardsAccountParams,
					reward: Balance,
				) {
					let rewards_account = bp_relayers::PayRewardFromAccount::<
						Balances,
						AccountId
					>::rewards_account(account_params);
					Self::deposit_account(rewards_account, reward);
				}

				fn deposit_account(account: AccountId, balance: Balance) {
					use frame_support::traits::fungible::Mutate;
					Balances::mint_into(&account, balance.saturating_add(ExistentialDeposit::get())).unwrap();
				}
			}

			use bridge_runtime_common::parachains_benchmarking::prepare_parachain_heads_proof;
			use pallet_bridge_parachains::benchmarking::Config as BridgeParachainsConfig;

			impl BridgeParachainsConfig<bridge_to_kusama_config::BridgeParachainKusamaInstance> for Runtime {
				fn parachains() -> Vec<bp_polkadot_core::parachains::ParaId> {
					use bp_runtime::Parachain;
					vec![bp_polkadot_core::parachains::ParaId(bp_bridge_hub_kusama::BridgeHubKusama::PARACHAIN_ID)]
				}

				fn prepare_parachain_heads_proof(
					parachains: &[bp_polkadot_core::parachains::ParaId],
					parachain_head_size: u32,
					proof_size: bp_runtime::StorageProofSize,
				) -> (
					pallet_bridge_parachains::RelayBlockNumber,
					pallet_bridge_parachains::RelayBlockHash,
					bp_polkadot_core::parachains::ParaHeadsProof,
					Vec<(bp_polkadot_core::parachains::ParaId, bp_polkadot_core::parachains::ParaHash)>,
				) {
					prepare_parachain_heads_proof::<Runtime, bridge_to_kusama_config::BridgeParachainKusamaInstance>(
						parachains,
						parachain_head_size,
						proof_size,
					)
				}
			}

			use bridge_runtime_common::messages_benchmarking::{
				prepare_message_delivery_proof_from_parachain,
				prepare_message_proof_from_parachain,
				generate_xcm_builder_bridge_message_sample,
			};
			use pallet_bridge_messages::benchmarking::{
				Config as BridgeMessagesConfig,
				MessageDeliveryProofParams,
				MessageProofParams,
			};

			impl BridgeMessagesConfig<bridge_to_kusama_config::WithBridgeHubKusamaMessagesInstance> for Runtime {
				fn is_relayer_rewarded(relayer: &Self::AccountId) -> bool {
					let bench_lane_id = <Self as BridgeMessagesConfig<bridge_to_kusama_config::WithBridgeHubKusamaMessagesInstance>>::bench_lane_id();
					let bridged_chain_id = bp_runtime::BRIDGE_HUB_KUSAMA_CHAIN_ID;
					pallet_bridge_relayers::Pallet::<Runtime>::relayer_reward(
						relayer,
						bp_relayers::RewardsAccountParams::new(
							bench_lane_id,
							bridged_chain_id,
							bp_relayers::RewardsAccountOwner::BridgedChain
						)
					).is_some()
				}

				fn prepare_message_proof(
					params: MessageProofParams,
				) -> (bridge_to_kusama_config::FromKusamaBridgeHubMessagesProof, Weight) {
					use cumulus_primitives_core::XcmpMessageSource;
					assert!(XcmpQueue::take_outbound_messages(usize::MAX).is_empty());
					ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(42.into());
					prepare_message_proof_from_parachain::<
						Runtime,
						bridge_to_kusama_config::BridgeGrandpaKusamaInstance,
						bridge_to_kusama_config::WithBridgeHubKusamaMessageBridge,
					>(params, generate_xcm_builder_bridge_message_sample(X2(GlobalConsensus(Polkadot), Parachain(42))))
				}

				fn prepare_message_delivery_proof(
					params: MessageDeliveryProofParams<AccountId>,
				) -> bridge_to_kusama_config::ToKusamaBridgeHubMessagesDeliveryProof {
					prepare_message_delivery_proof_from_parachain::<
						Runtime,
						bridge_to_kusama_config::BridgeGrandpaKusamaInstance,
						bridge_to_kusama_config::WithBridgeHubKusamaMessageBridge,
					>(params)
				}

				fn is_message_successfully_dispatched(_nonce: bp_messages::MessageNonce) -> bool {
					use cumulus_primitives_core::XcmpMessageSource;
					!XcmpQueue::take_outbound_messages(usize::MAX).is_empty()
				}
			}

			let whitelist: Vec<TrackedStorageKey> = vec![
				// Block Number
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac").to_vec().into(),
				// Total Issuance
				hex_literal::hex!("c2261276cc9d1f8598ea4b6a74b15c2f57c875e4cff74148e4628f264b974c80").to_vec().into(),
				// Execution Phase
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef7ff553b5a9862a516939d82b3d3d8661a").to_vec().into(),
				// Event Count
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef70a98fdbe9ce6c55837576c60c7af3850").to_vec().into(),
				// System Events
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7").to_vec().into(),
			];

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			add_benchmarks!(params, batches);

			Ok(batches)
		}
	}
}

cumulus_pallet_parachain_system::register_validate_block! {
	Runtime = Runtime,
	BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
}
