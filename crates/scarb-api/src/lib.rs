use anyhow::{anyhow, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use scarb_metadata::{Metadata, PackageId, PackageMetadata, TargetMetadata};
use semver::VersionReq;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::str::FromStr;
use universal_sierra_compiler_api::{compile_sierra_at_path, SierraType};

pub use command::*;
use shared::print::print_as_warning;

mod command;
pub mod metadata;
pub mod version;

const INTEGRATION_TEST_TYPE: &str = "integration";

#[derive(Deserialize, Debug, PartialEq, Clone)]
struct StarknetArtifacts {
    version: u32,
    contracts: Vec<StarknetContract>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, PartialEq, Clone)]
struct StarknetContract {
    id: String,
    package_name: String,
    contract_name: String,
    artifacts: StarknetContractArtifactPaths,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, PartialEq, Clone)]
struct StarknetContractArtifactPaths {
    sierra: Utf8PathBuf,
}

/// Contains compiled Starknet artifacts
#[derive(Debug, PartialEq, Clone)]
pub struct StarknetContractArtifacts {
    /// Compiled sierra code
    pub sierra: String,
    /// Compiled casm code
    pub casm: String,
}

impl StarknetContractArtifacts {
    fn from_scarb_contract_artifact(
        starknet_contract: &StarknetContract,
        base_path: &Utf8Path,
    ) -> Result<Self> {
        let sierra_path = base_path.join(starknet_contract.artifacts.sierra.clone());
        let sierra = fs::read_to_string(sierra_path)?;

        let casm = compile_sierra_at_path(
            starknet_contract.artifacts.sierra.as_str(),
            Some(base_path.as_std_path()),
            &SierraType::Contract,
        )?;

        Ok(Self { sierra, casm })
    }
}

/// Get deserialized contents of `starknet_artifacts.json` file generated by Scarb
///
/// # Arguments
///
/// * `path` - A path to `starknet_artifacts.json` file.
fn artifacts_for_package(path: &Utf8Path) -> Result<StarknetArtifacts> {
    let starknet_artifacts =
        fs::read_to_string(path).with_context(|| format!("Failed to read {path:?} contents"))?;
    let starknet_artifacts: StarknetArtifacts =
        serde_json::from_str(starknet_artifacts.as_str())
            .with_context(|| format!("Failed to parse {path:?} contents. Make sure you have enabled sierra code generation in Scarb.toml"))?;
    Ok(starknet_artifacts)
}

#[derive(PartialEq, Debug)]
struct StarknetArtifactsFiles {
    base_file: Utf8PathBuf,
    other_files: Vec<Utf8PathBuf>,
}

impl StarknetArtifactsFiles {
    fn load_contracts_artifacts(
        self,
    ) -> Result<HashMap<String, (StarknetContractArtifacts, Utf8PathBuf)>> {
        let mut base_artifacts = load_contracts_artifacts_and_source_sierra_paths(&self.base_file)?;

        let compiled_artifacts = self
            .other_files
            .par_iter()
            .map(load_contracts_artifacts_and_source_sierra_paths)
            .collect::<Result<Vec<_>>>()?;

        for artifact in compiled_artifacts {
            for (key, value) in artifact {
                base_artifacts.entry(key).or_insert(value);
            }
        }

        Ok(base_artifacts)
    }
}

/// Constructs `StarknetArtifactsFiles` from contracts built using test target.
///
/// If artifacts with `test_type` of `INTEGRATION_TEST_TYPE` are present, we use them base path
/// and extend with paths to other artifacts.
/// If `INTEGRATION_TEST_TYPE` is not present, we take first artifacts found.
fn get_starknet_artifacts_paths_from_test_targets(
    target_dir: &Utf8Path,
    test_targets: &HashMap<String, &TargetMetadata>,
) -> Option<StarknetArtifactsFiles> {
    #[derive(PartialEq, Debug, Clone)]
    struct ContractArtifactData {
        path: Utf8PathBuf,
        test_type: Option<String>,
    }

    let artifact = |name: &str, metadata: &TargetMetadata| -> Option<ContractArtifactData> {
        let path = format!("{name}.test.starknet_artifacts.json");
        let path = target_dir.join(&path);
        let path = if path.exists() {
            Some(path)
        } else {
            print_as_warning(&anyhow!(
                "File = {path} missing when it should be existing, perhaps due to Scarb problem."
            ));
            None
        };

        let test_type = metadata
            .params
            .get("test-type")
            .and_then(|value| value.as_str())
            .map(ToString::to_string);

        path.map(|path| ContractArtifactData {
            path: Utf8PathBuf::from_str(path.as_str()).unwrap(),
            test_type,
        })
    };

    let artifacts = test_targets
        .iter()
        .filter_map(|(target_name, metadata)| artifact(target_name, metadata))
        .collect::<Vec<_>>();

    let base_artifact = artifacts
        .iter()
        .find(|paths| paths.test_type == Some(INTEGRATION_TEST_TYPE.to_string()))
        .cloned()
        .or_else(|| artifacts.first().cloned());

    if let Some(base_artifact) = base_artifact {
        let other_artifacts = artifacts
            .into_iter()
            .filter(|artifact| artifact != &base_artifact)
            .map(|artifact| artifact.path)
            .collect();

        Some(StarknetArtifactsFiles {
            base_file: base_artifact.path.clone(),
            other_files: other_artifacts,
        })
    } else {
        None
    }
}

/// Try getting the path to `starknet_artifacts.json` file that is generated by `scarb build` command
/// If the file is not present, `None` is returned.
fn get_starknet_artifacts_path(
    target_dir: &Utf8Path,
    target_name: &str,
) -> Option<StarknetArtifactsFiles> {
    let path = format!("{target_name}.starknet_artifacts.json");
    let path = target_dir.join(&path);
    let path = if path.exists() {
        Some(path)
    } else {
        print_as_warning(&anyhow!(
            "File = {path} missing.\
        This is most likely caused by `[[target.starknet-contract]]` being undefined in Scarb.toml\
        No contracts will be available for deployment"
        ));
        None
    };

    path.map(|path| StarknetArtifactsFiles {
        base_file: path,
        other_files: vec![],
    })
}

/// Get the map with `StarknetContractArtifacts` for the given package
pub fn get_contracts_artifacts_and_source_sierra_paths(
    target_dir: &Utf8Path,
    package: &PackageMetadata,
    use_test_target_contracts: bool,
) -> Result<HashMap<String, (StarknetContractArtifacts, Utf8PathBuf)>> {
    let starknet_artifact_files = if use_test_target_contracts {
        let test_targets = test_targets_by_name(package);
        get_starknet_artifacts_paths_from_test_targets(target_dir, &test_targets)
    } else {
        let starknet_target_name = package
            .targets
            .iter()
            .find(|target| target.kind == "starknet-contract")
            .map(|target| target.name.clone());
        starknet_target_name.and_then(|starknet_target_name| {
            get_starknet_artifacts_path(target_dir, starknet_target_name.as_str())
        })
    };

    if let Some(starknet_artifact_files) = starknet_artifact_files {
        starknet_artifact_files.load_contracts_artifacts()
    } else {
        Ok(HashMap::default())
    }
}

fn load_contracts_artifacts_and_source_sierra_paths(
    contracts_path: &Utf8PathBuf,
) -> Result<HashMap<String, (StarknetContractArtifacts, Utf8PathBuf)>> {
    let base_path = contracts_path
        .parent()
        .ok_or_else(|| anyhow!("Failed to get parent for path = {}", &contracts_path))?;
    let artifacts = artifacts_for_package(contracts_path)?;
    let mut map = HashMap::new();

    for ref contract in artifacts.contracts {
        let name = contract.contract_name.clone();
        let contract_artifacts =
            StarknetContractArtifacts::from_scarb_contract_artifact(contract, base_path)?;

        let sierra_path = base_path.join(contract.artifacts.sierra.clone());

        map.insert(name.clone(), (contract_artifacts, sierra_path));
    }
    Ok(map)
}

#[must_use]
pub fn target_dir_for_workspace(metadata: &Metadata) -> Utf8PathBuf {
    metadata
        .target_dir
        .clone()
        .unwrap_or_else(|| metadata.workspace.root.join("target"))
}

/// Get a name of the given package
pub fn name_for_package(metadata: &Metadata, package: &PackageId) -> Result<String> {
    let package = metadata
        .get_package(package)
        .ok_or_else(|| anyhow!("Failed to find metadata for package = {package}"))?;

    Ok(package.name.clone())
}

/// Checks if the specified package has version compatible with the specified requirement
pub fn package_matches_version_requirement(
    metadata: &Metadata,
    name: &str,
    version_req: &VersionReq,
) -> Result<bool> {
    let mut packages = metadata
        .packages
        .iter()
        .filter(|package| package.name == name);

    match (packages.next(), packages.next()) {
        (Some(package), None) => Ok(version_req.matches(&package.version)),
        (None, None) => Err(anyhow!("Package {name} is not present in dependencies.")),
        _ => Err(anyhow!("Package {name} is duplicated in dependencies")),
    }
}

/// collecting by name allow us to dedup targets
/// we do it because they use same sierra and we display them without distinction anyway
#[must_use]
pub fn test_targets_by_name(package: &PackageMetadata) -> HashMap<String, &TargetMetadata> {
    fn test_target_name(target: &TargetMetadata) -> String {
        // this is logic copied from scarb: https://github.com/software-mansion/scarb/blob/90ab01cb6deee48210affc2ec1dc94d540ab4aea/extensions/scarb-cairo-test/src/main.rs#L115
        target
            .params
            .get("group-id") // by unit tests grouping
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .unwrap_or(target.name.clone()) // else by integration test name
    }

    package
        .targets
        .iter()
        .filter(|target| target.kind == "test")
        .map(|target| (test_target_name(target), target))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::MetadataCommandExt;
    use assert_fs::fixture::{FileWriteStr, PathChild, PathCopy};
    use assert_fs::prelude::FileTouch;
    use assert_fs::TempDir;
    use camino::Utf8PathBuf;
    use indoc::{formatdoc, indoc};
    use std::str::FromStr;

    fn setup_package(package_name: &str) -> TempDir {
        let temp = TempDir::new().unwrap();
        temp.copy_from(
            format!("tests/data/{package_name}"),
            &["**/*.cairo", "**/*.toml"],
        )
        .unwrap();
        temp.copy_from("../../", &[".tool-versions"]).unwrap();

        let snforge_std_path = Utf8PathBuf::from_str("../../snforge_std")
            .unwrap()
            .canonicalize_utf8()
            .unwrap()
            .to_string()
            .replace('\\', "/");

        let manifest_path = temp.child("Scarb.toml");
        manifest_path
            .write_str(&formatdoc!(
                r#"
                [package]
                name = "{}"
                version = "0.1.0"

                [dependencies]
                starknet = "2.4.0"
                snforge_std = {{ path = "{}" }}

                [[target.starknet-contract]]

                [[tool.snforge.fork]]
                name = "FIRST_FORK_NAME"
                url = "http://some.rpc.url"
                block_id.number = "1"

                [[tool.snforge.fork]]
                name = "SECOND_FORK_NAME"
                url = "http://some.rpc.url"
                block_id.hash = "1"

                [[tool.snforge.fork]]
                name = "THIRD_FORK_NAME"
                url = "http://some.rpc.url"
                block_id.tag = "latest"
                "#,
                package_name,
                snforge_std_path
            ))
            .unwrap();

        temp
    }

    #[test]
    fn get_starknet_artifacts_path_for_standard_build() {
        let temp = setup_package("basic_package");

        ScarbCommand::new_with_stdio()
            .current_dir(temp.path())
            .arg("build")
            .run()
            .unwrap();

        let path = get_starknet_artifacts_path(
            &Utf8PathBuf::from_path_buf(temp.to_path_buf().join("target").join("dev")).unwrap(),
            "basic_package",
        )
        .unwrap();

        assert_eq!(
            path,
            StarknetArtifactsFiles {
                base_file: Utf8PathBuf::from_path_buf(
                    temp.path()
                        .join("target/dev/basic_package.starknet_artifacts.json")
                )
                .unwrap(),
                other_files: vec![]
            }
        );
    }

    #[test]
    #[cfg_attr(not(feature = "scarb_2_8_3"), ignore)]
    fn get_starknet_artifacts_path_for_test_build() {
        let temp = setup_package("basic_package");

        ScarbCommand::new_with_stdio()
            .current_dir(temp.path())
            .arg("build")
            .arg("--test")
            .run()
            .unwrap();

        let metadata = ScarbCommand::metadata()
            .current_dir(temp.path())
            .run()
            .unwrap();

        let package = metadata
            .packages
            .iter()
            .find(|p| p.name == "basic_package")
            .unwrap();

        let path = get_starknet_artifacts_paths_from_test_targets(
            &Utf8PathBuf::from_path_buf(temp.join("target").join("dev")).unwrap(),
            &test_targets_by_name(package),
        )
        .unwrap();

        assert_eq!(
            path,
            StarknetArtifactsFiles {
                base_file: Utf8PathBuf::from_path_buf(
                    temp.path()
                        .join("target/dev/basic_package_unittest.test.starknet_artifacts.json")
                )
                .unwrap(),
                other_files: vec![],
            }
        );
    }

    #[test]
    #[cfg_attr(not(feature = "scarb_2_8_3"), ignore)]
    fn get_starknet_artifacts_path_for_test_build_when_integration_tests_exist() {
        let temp = setup_package("basic_package");
        let tests_dir = temp.join("tests");
        fs::create_dir(&tests_dir).unwrap();

        temp.child(tests_dir.join("test.cairo"))
            .write_str(indoc!(
                r"
                #[test]
                fn mock_test() {
                    assert!(true);
                }
            "
            ))
            .unwrap();

        ScarbCommand::new_with_stdio()
            .current_dir(temp.path())
            .arg("build")
            .arg("--test")
            .run()
            .unwrap();

        let metadata = ScarbCommand::metadata()
            .current_dir(temp.path())
            .run()
            .unwrap();

        let package = metadata
            .packages
            .iter()
            .find(|p| p.name == "basic_package")
            .unwrap();

        let path = get_starknet_artifacts_paths_from_test_targets(
            &Utf8PathBuf::from_path_buf(temp.to_path_buf().join("target").join("dev")).unwrap(),
            &test_targets_by_name(package),
        )
        .unwrap();

        assert_eq!(
            path,
            StarknetArtifactsFiles {
                base_file: Utf8PathBuf::from_path_buf(
                    temp.path().join(
                        "target/dev/basic_package_integrationtest.test.starknet_artifacts.json"
                    )
                )
                .unwrap(),
                other_files: vec![Utf8PathBuf::from_path_buf(
                    temp.path()
                        .join("target/dev/basic_package_unittest.test.starknet_artifacts.json")
                )
                .unwrap(),]
            },
        );
    }

    #[test]
    fn package_matches_version_requirement_test() {
        let temp = setup_package("basic_package");

        let manifest_path = temp.child("Scarb.toml");
        manifest_path
            .write_str(&formatdoc!(
                r#"
                [package]
                name = "version_checker"
                version = "0.1.0"

                [[target.starknet-contract]]
                sierra = true

                [dependencies]
                starknet = "2.5.4"
                "#,
            ))
            .unwrap();

        let scarb_metadata = ScarbCommand::metadata()
            .inherit_stderr()
            .current_dir(temp.path())
            .run()
            .unwrap();

        assert!(package_matches_version_requirement(
            &scarb_metadata,
            "starknet",
            &VersionReq::parse("2.5").unwrap(),
        )
        .unwrap());

        assert!(package_matches_version_requirement(
            &scarb_metadata,
            "not_existing",
            &VersionReq::parse("2.5").unwrap(),
        )
        .is_err());

        assert!(!package_matches_version_requirement(
            &scarb_metadata,
            "starknet",
            &VersionReq::parse("2.8").unwrap(),
        )
        .unwrap());
    }

    #[test]
    fn get_starknet_artifacts_path_for_project_with_different_package_and_target_name() {
        let temp = setup_package("basic_package");

        let snforge_std_path = Utf8PathBuf::from_str("../../snforge_std")
            .unwrap()
            .canonicalize_utf8()
            .unwrap()
            .to_string()
            .replace('\\', "/");

        let scarb_path = temp.child("Scarb.toml");
        scarb_path
            .write_str(&formatdoc!(
                r#"
                [package]
                name = "basic_package"
                version = "0.1.0"

                [dependencies]
                starknet = "2.4.0"
                snforge_std = {{ path = "{}" }}

                [[target.starknet-contract]]
                name = "essa"
                sierra = true
                "#,
                snforge_std_path
            ))
            .unwrap();

        ScarbCommand::new_with_stdio()
            .current_dir(temp.path())
            .arg("build")
            .run()
            .unwrap();

        let path = get_starknet_artifacts_path(
            &Utf8PathBuf::from_path_buf(temp.to_path_buf().join("target").join("dev")).unwrap(),
            "essa",
        )
        .unwrap();

        assert_eq!(
            path,
            StarknetArtifactsFiles {
                base_file: Utf8PathBuf::from_path_buf(
                    temp.path().join("target/dev/essa.starknet_artifacts.json")
                )
                .unwrap(),
                other_files: vec![]
            }
        );
    }

    #[test]
    fn get_starknet_artifacts_path_for_project_without_starknet_target() {
        let temp = setup_package("empty_lib");

        let manifest_path = temp.child("Scarb.toml");
        manifest_path
            .write_str(indoc!(
                r#"
            [package]
            name = "empty_lib"
            version = "0.1.0"
            "#,
            ))
            .unwrap();

        ScarbCommand::new_with_stdio()
            .current_dir(temp.path())
            .arg("build")
            .run()
            .unwrap();

        let path = get_starknet_artifacts_path(
            &Utf8PathBuf::from_path_buf(temp.to_path_buf().join("target").join("dev")).unwrap(),
            "empty_lib",
        );
        assert!(path.is_none());
    }

    #[test]
    fn get_starknet_artifacts_path_for_project_without_scarb_build() {
        let temp = setup_package("basic_package");

        let path = get_starknet_artifacts_path(
            &Utf8PathBuf::from_path_buf(temp.to_path_buf().join("target").join("dev")).unwrap(),
            "basic_package",
        );
        assert!(path.is_none());
    }

    #[test]
    fn parsing_starknet_artifacts() {
        let temp = setup_package("basic_package");

        ScarbCommand::new_with_stdio()
            .current_dir(temp.path())
            .arg("build")
            .run()
            .unwrap();

        let artifacts_path = temp
            .path()
            .join("target/dev/basic_package.starknet_artifacts.json");
        let artifacts_path = Utf8PathBuf::from_path_buf(artifacts_path).unwrap();

        let artifacts = artifacts_for_package(&artifacts_path).unwrap();

        assert!(!artifacts.contracts.is_empty());
    }

    #[test]
    fn parsing_starknet_artifacts_on_invalid_file() {
        let temp = TempDir::new().unwrap();
        temp.copy_from("../../", &[".tool-versions"]).unwrap();
        let path = temp.child("wrong.json");
        path.touch().unwrap();
        path.write_str("\"aa\": {}").unwrap();
        let artifacts_path = Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap();

        let result = artifacts_for_package(&artifacts_path);
        let err = result.unwrap_err();

        assert!(err.to_string().contains(&format!("Failed to parse {artifacts_path:?} contents. Make sure you have enabled sierra code generation in Scarb.toml")));
    }

    #[test]
    fn get_contracts() {
        let temp = setup_package("basic_package");

        ScarbCommand::new_with_stdio()
            .current_dir(temp.path())
            .arg("build")
            .run()
            .unwrap();

        let metadata = ScarbCommand::metadata()
            .inherit_stderr()
            .manifest_path(temp.join("Scarb.toml"))
            .run()
            .unwrap();

        let target_dir = target_dir_for_workspace(&metadata).join("dev");
        let package = metadata.packages.first().unwrap();

        let contracts =
            get_contracts_artifacts_and_source_sierra_paths(target_dir.as_path(), package, false)
                .unwrap();

        assert!(contracts.contains_key("ERC20"));
        assert!(contracts.contains_key("HelloStarknet"));

        let sierra_contents_erc20 =
            fs::read_to_string(temp.join("target/dev/basic_package_ERC20.contract_class.json"))
                .unwrap();

        let contract = contracts.get("ERC20").unwrap();
        assert_eq!(&sierra_contents_erc20, &contract.0.sierra);
        assert!(!contract.0.casm.is_empty());

        let sierra_contents_erc20 = fs::read_to_string(
            temp.join("target/dev/basic_package_HelloStarknet.contract_class.json"),
        )
        .unwrap();
        let contract = contracts.get("HelloStarknet").unwrap();
        assert_eq!(&sierra_contents_erc20, &contract.0.sierra);
        assert!(!contract.0.casm.is_empty());
    }

    #[test]
    fn get_name_for_package() {
        let temp = setup_package("basic_package");
        let scarb_metadata = ScarbCommand::metadata()
            .inherit_stderr()
            .current_dir(temp.path())
            .run()
            .unwrap();

        let package_name =
            name_for_package(&scarb_metadata, &scarb_metadata.workspace.members[0]).unwrap();

        assert_eq!(&package_name, "basic_package");
    }
}
