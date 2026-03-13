use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use alloy::network::EthereumWallet;
use anyhow::{Context, Result};

pub struct Wallet {
    pub address: Address,
    ethereum_wallet: EthereumWallet,
}

impl Wallet {
    /// Load wallet from BASALT_PRIVATE_KEY environment variable
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("BASALT_PRIVATE_KEY")
            .context("BASALT_PRIVATE_KEY env var not set")?;
        let signer: PrivateKeySigner = key.parse()
            .context("Failed to parse BASALT_PRIVATE_KEY as a private key")?;
        let address = signer.address();
        let ethereum_wallet = EthereumWallet::from(signer);
        Ok(Self { address, ethereum_wallet })
    }

    /// Create wallet directly from a signer (useful for testing with Anvil keys)
    pub fn from_signer(signer: PrivateKeySigner) -> Self {
        let address = signer.address();
        let ethereum_wallet = EthereumWallet::from(signer);
        Self { address, ethereum_wallet }
    }

    /// Get a clone of the EthereumWallet for use with ProviderBuilder
    pub fn ethereum_wallet(&self) -> EthereumWallet {
        self.ethereum_wallet.clone()
    }
}
