//! Solana DAS (Digital Asset Standard) API client.
//!
//! The DAS API (popularized by Helius / Triton; the JSON-RPC spec is
//! provider-agnostic) lets us walk all assets in a Metaplex NFT collection or all
//! accounts of a SPL/Token-22 mint by paging through `getAssetsByGroup` and
//! `getTokenAccountsByMint` results.
//!
//! ## Why DAS over `getProgramAccounts`
//!
//! `getProgramAccounts` against the Token-22 program with a mint filter would also
//! work, but it's gated on most public RPCs because the result set is enormous. DAS
//! does the heavy lifting on the provider side and pages results back to us at <= 1000
//! entries per request.
//!
//! ## Trait abstraction
//!
//! The [`DasClient`] trait makes the network-talking logic mockable. Production uses
//! [`HttpDasClient`]; tests use a hand-rolled fake that returns canned responses.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// One Metaplex NFT asset returned by `getAssetsByGroup`.
///
/// We only care about the `ownership.owner` field for the megadrop snapshot — the
/// asset id, name, etc. are dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DasAsset {
    /// The asset's mint address (informational; not used in allocation math).
    pub asset_id: Pubkey,
    /// The current owner of the asset (its NFT holder).
    pub owner: Pubkey,
}

/// One token account returned by `getTokenAccountsByMint`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DasTokenAccount {
    /// Token account address (informational).
    pub address: Pubkey,
    /// Owner of the token account (the holder we group by).
    pub owner: Pubkey,
    /// Balance of the mint in this token account, in raw mint units (no decimals).
    pub amount: u64,
}

/// DAS-style client. Behind a trait so tests can supply a mock that returns canned
/// pages without touching mainnet RPC.
pub trait DasClient {
    /// Fetch every asset whose verified collection equals `collection`. Implementors
    /// should page through the DAS API until exhausted.
    fn fetch_assets_by_collection(&self, collection: &Pubkey) -> Result<Vec<DasAsset>>;

    /// Fetch every Token-22 token account holding a balance of `mint`. Should page
    /// through the API until exhausted.
    fn fetch_token_accounts_by_mint(
        &self,
        mint: &Pubkey,
    ) -> Result<Vec<DasTokenAccount>>;
}

/// HTTP DAS client backed by `reqwest::blocking`. One client per snapshot run.
pub struct HttpDasClient {
    rpc_url: String,
    http: reqwest::blocking::Client,
}

impl HttpDasClient {
    /// Construct a new client pointing at `rpc_url` (e.g.
    /// `https://api.mainnet-beta.solana.com` or a Helius endpoint with the API key
    /// embedded).
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest blocking client"),
        }
    }
}

/// JSON-RPC envelope for outgoing requests.
#[derive(Serialize)]
struct RpcRequest<'a, P: Serialize> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: P,
}

/// JSON-RPC envelope for incoming responses.
///
/// `#[serde(bound)]` is required because `result: Option<R>` is wrapped in a
/// `#[serde(default)]` attribute — serde's default `derive(Deserialize)` for a
/// generic struct would otherwise demand `R: Default`. Asking only for
/// `Deserialize<'de>` is what we actually need.
#[derive(Deserialize)]
#[serde(bound(deserialize = "R: serde::de::DeserializeOwned"))]
#[allow(dead_code)] // `jsonrpc` / `id` are read off the wire for completeness
struct RpcResponse<R> {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: u64,
    #[serde(default = "none_default")]
    result: Option<R>,
    #[serde(default)]
    error: Option<RpcError>,
}

fn none_default<T>() -> Option<T> {
    None
}

#[derive(Deserialize, Debug)]
struct RpcError {
    code: i64,
    message: String,
}

/// `getAssetsByGroup` request shape (DAS spec).
#[derive(Serialize)]
struct AssetsByGroupParams<'a> {
    #[serde(rename = "groupKey")]
    group_key: &'a str,
    #[serde(rename = "groupValue")]
    group_value: String,
    page: u32,
    limit: u32,
}

/// Top-level result for `getAssetsByGroup`.
#[derive(Deserialize, Debug)]
#[allow(dead_code)] // `limit` / `page` are read off the wire for completeness
struct AssetsByGroupResult {
    items: Vec<RawDasAsset>,
    #[serde(default)]
    total: u32,
    #[serde(default)]
    limit: u32,
    #[serde(default)]
    page: u32,
}

#[derive(Deserialize, Debug)]
struct RawDasAsset {
    id: String,
    ownership: RawOwnership,
}

#[derive(Deserialize, Debug)]
struct RawOwnership {
    owner: String,
}

/// `getTokenAccounts` request shape (DAS spec — see Helius docs).
#[derive(Serialize)]
struct TokenAccountsParams<'a> {
    mint: String,
    page: u32,
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // `cursor` reserved for future cursor-based pagination switch
struct TokenAccountsResult {
    #[serde(default)]
    token_accounts: Vec<RawTokenAccount>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    total: u32,
}

#[derive(Deserialize, Debug)]
struct RawTokenAccount {
    address: String,
    owner: String,
    amount: u64,
}

impl HttpDasClient {
    fn rpc_call<P: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R> {
        let body = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };
        let resp: RpcResponse<R> = self
            .http
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .with_context(|| format!("DAS RPC POST to {} failed", self.rpc_url))?
            .error_for_status()
            .with_context(|| format!("DAS RPC HTTP error on {method}"))?
            .json()
            .with_context(|| format!("DAS RPC JSON decode error on {method}"))?;
        if resp.jsonrpc != "2.0" {
            return Err(anyhow!("DAS RPC returned unexpected jsonrpc field"));
        }
        if let Some(err) = resp.error {
            return Err(anyhow!(
                "DAS RPC error code {} on {}: {}",
                err.code,
                method,
                err.message
            ));
        }
        resp.result
            .ok_or_else(|| anyhow!("DAS RPC {} returned no result", method))
    }
}

fn parse_pubkey(s: &str) -> Result<Pubkey> {
    s.parse::<Pubkey>()
        .with_context(|| format!("invalid base58 pubkey {s:?}"))
}

impl DasClient for HttpDasClient {
    fn fetch_assets_by_collection(&self, collection: &Pubkey) -> Result<Vec<DasAsset>> {
        let mut all: Vec<DasAsset> = Vec::new();
        let limit: u32 = 1_000;
        let mut page: u32 = 1;
        loop {
            let params = AssetsByGroupParams {
                group_key: "collection",
                group_value: collection.to_string(),
                page,
                limit,
            };
            let result: AssetsByGroupResult =
                self.rpc_call("getAssetsByGroup", params)?;
            let n = result.items.len();
            for raw in result.items {
                let asset_id = parse_pubkey(&raw.id)?;
                let owner = parse_pubkey(&raw.ownership.owner)?;
                all.push(DasAsset { asset_id, owner });
            }
            // Termination condition: page returned fewer than `limit` items, OR we've
            // reached `total` (when DAS reports it). Defensive: pin a hard upper
            // bound on page count to catch a misbehaving provider that paginates
            // forever.
            if (n as u32) < limit {
                break;
            }
            if result.total > 0 && (all.len() as u32) >= result.total {
                break;
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| anyhow!("DAS pagination overflowed page counter"))?;
            if page > 10_000 {
                return Err(anyhow!(
                    "DAS pagination aborted at page 10000 — provider misbehaved"
                ));
            }
        }
        Ok(all)
    }

    fn fetch_token_accounts_by_mint(
        &self,
        mint: &Pubkey,
    ) -> Result<Vec<DasTokenAccount>> {
        let mut all: Vec<DasTokenAccount> = Vec::new();
        let limit: u32 = 1_000;
        let mut page: u32 = 1;
        loop {
            let params = TokenAccountsParams {
                mint: mint.to_string(),
                page,
                limit,
                cursor: None,
            };
            let result: TokenAccountsResult =
                self.rpc_call("getTokenAccounts", params)?;
            let n = result.token_accounts.len();
            for raw in result.token_accounts {
                let address = parse_pubkey(&raw.address)?;
                let owner = parse_pubkey(&raw.owner)?;
                all.push(DasTokenAccount {
                    address,
                    owner,
                    amount: raw.amount,
                });
            }
            if (n as u32) < limit {
                break;
            }
            if result.total > 0 && (all.len() as u32) >= result.total {
                break;
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| anyhow!("DAS pagination overflowed page counter"))?;
            if page > 10_000 {
                return Err(anyhow!(
                    "DAS pagination aborted at page 10000 — provider misbehaved"
                ));
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Mock client returning canned responses. Used in `snapshot.rs` tests.
    pub struct MockDasClient {
        pub assets: Vec<DasAsset>,
        pub token_accounts: Vec<DasTokenAccount>,
        pub asset_calls: RefCell<u32>,
        pub token_calls: RefCell<u32>,
    }

    impl MockDasClient {
        pub fn new() -> Self {
            Self {
                assets: Vec::new(),
                token_accounts: Vec::new(),
                asset_calls: RefCell::new(0),
                token_calls: RefCell::new(0),
            }
        }
    }

    impl DasClient for MockDasClient {
        fn fetch_assets_by_collection(
            &self,
            _collection: &Pubkey,
        ) -> Result<Vec<DasAsset>> {
            *self.asset_calls.borrow_mut() += 1;
            Ok(self.assets.clone())
        }

        fn fetch_token_accounts_by_mint(
            &self,
            _mint: &Pubkey,
        ) -> Result<Vec<DasTokenAccount>> {
            *self.token_calls.borrow_mut() += 1;
            Ok(self.token_accounts.clone())
        }
    }

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn mock_client_returns_canned_assets() {
        let mut mock = MockDasClient::new();
        mock.assets = vec![DasAsset {
            asset_id: pk(1),
            owner: pk(2),
        }];
        let got = mock.fetch_assets_by_collection(&pk(99)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].owner, pk(2));
    }

    #[test]
    fn mock_client_returns_canned_token_accounts() {
        let mut mock = MockDasClient::new();
        mock.token_accounts = vec![DasTokenAccount {
            address: pk(10),
            owner: pk(11),
            amount: 1_000_000,
        }];
        let got = mock.fetch_token_accounts_by_mint(&pk(99)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].amount, 1_000_000);
    }
}
