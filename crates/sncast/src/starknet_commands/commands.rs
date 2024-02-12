use anyhow;
use starknet::core::types::StarknetError::{
    BlockNotFound, ClassAlreadyDeclared, ClassHashNotFound, CompilationFailed,
    CompiledClassHashMismatch, ContractClassSizeIsTooLarge, ContractError, ContractNotFound,
    DuplicateTx, FailedToReceiveTransaction, InsufficientAccountBalance, InsufficientMaxFee,
    InvalidTransactionIndex, InvalidTransactionNonce, NonAccount, TransactionExecutionError,
    TransactionHashNotFound, UnsupportedContractClassVersion, UnsupportedTxVersion,
    ValidationFailure,
};
use starknet::providers::ProviderError;
use starknet::providers::ProviderError::StarknetError;
use sncast::{ErrorData, TransactionError, WaitForTransactionError};

#[derive(Debug)]
pub enum StarknetCommandError {
    Unhandleable(anyhow::Error),
    ContractArtifactsNotFound(ContractArtifactsNotFoundData),
    TransactionError(TransactionError),
    Handleable(ProviderError),
}

#[derive(Debug)]
pub struct ContractArtifactsNotFoundData {
    pub(crate) contract_name: String,
}

impl From<anyhow::Error> for StarknetCommandError {
    fn from(value: anyhow::Error) -> Self {
        StarknetCommandError::Unhandleable(value)
    }
}

impl From<WaitForTransactionError> for StarknetCommandError {
    fn from(value: WaitForTransactionError) -> Self {
        match value {
            WaitForTransactionError::TransactionError(error) => StarknetCommandError::TransactionError(error),
            WaitForTransactionError::Other(error) => StarknetCommandError::Unhandleable(error),
        }
    }
}

pub fn handle_starknet_command_error(error: StarknetCommandError) -> anyhow::Error {
    match error {
        StarknetCommandError::Unhandleable(error) => error,
        StarknetCommandError::ContractArtifactsNotFound(ContractArtifactsNotFoundData{contract_name}) => anyhow::anyhow!("Failed to find {contract_name} artifact in starknet_artifacts.json file. Please make sure you have specified correct package using `--package` flag and that you have enabled sierra and casm code generation in Scarb.toml"),
        StarknetCommandError::Handleable(error) => match error {
            StarknetError(FailedToReceiveTransaction) => {
                anyhow::anyhow!("Node failed to receive transaction")
            }
            StarknetError(ContractNotFound) => {
                anyhow::anyhow!("There is no contract at the specified address")
            }
            StarknetError(BlockNotFound) => anyhow::anyhow!("Block was not found"),
            StarknetError(TransactionHashNotFound) => {
                anyhow::anyhow!("Transaction with provided hash was not found (does not exist)")
            }
            StarknetError(InvalidTransactionIndex) => {
                anyhow::anyhow!("There is no transaction with such an index")
            }
            StarknetError(ClassHashNotFound) => {
                anyhow::anyhow!("Provided class hash does not exist")
            }
            StarknetError(ContractError(err)) => {
                anyhow::anyhow!("An error occurred in the called contract = {err:?}")
            }
            StarknetError(InvalidTransactionNonce) => anyhow::anyhow!("Invalid transaction nonce"),
            StarknetError(InsufficientMaxFee) => {
                anyhow::anyhow!("Max fee is smaller than the minimal transaction cost")
            }
            StarknetError(InsufficientAccountBalance) => {
                anyhow::anyhow!("Account balance is too small to cover transaction fee")
            }
            StarknetError(ClassAlreadyDeclared) => {
                anyhow::anyhow!("Contract with the same class hash is already declared")
            }
            StarknetError(TransactionExecutionError(err)) => {
                anyhow::anyhow!("Transaction execution error = {err:?}")
            }
            StarknetError(ValidationFailure(err)) => {
                anyhow::anyhow!("Contract failed the validation = {err}")
            }
            StarknetError(CompilationFailed) => {
                anyhow::anyhow!("Contract failed to compile in starknet")
            }
            StarknetError(ContractClassSizeIsTooLarge) => {
                anyhow::anyhow!("Contract class size is too large")
            }
            StarknetError(NonAccount) => anyhow::anyhow!("No account"),
            StarknetError(DuplicateTx) => anyhow::anyhow!("Transaction already exists"),
            StarknetError(CompiledClassHashMismatch) => {
                anyhow::anyhow!("Compiled class hash mismatch")
            }
            StarknetError(UnsupportedTxVersion) => {
                anyhow::anyhow!("Unsupported transaction version")
            }
            StarknetError(UnsupportedContractClassVersion) => {
                anyhow::anyhow!("Unsupported contract class version")
            }
            _ => anyhow::anyhow!("Unknown RPC error"),
        },
        StarknetCommandError::TransactionError(error) => match error {
            TransactionError::Rejected => anyhow::anyhow!("Transaction has been rejected"),
            TransactionError::Reverted( ErrorData { data: reason } ) => anyhow::anyhow!("Transaction has been reverted = {reason}"),
        }
    }
}
