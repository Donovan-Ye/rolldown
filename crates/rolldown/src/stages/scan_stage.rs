use std::sync::Arc;

use index_vec::IndexVec;
use rolldown_common::{EntryPoint, ImportKind, NormalModuleId};
use rolldown_error::BuildError;
use rolldown_fs::FileSystem;
use rolldown_oxc_utils::OxcProgram;
use rolldown_plugin::HookResolveIdArgsOptions;
use rolldown_utils::block_on_spawn_all;

use crate::{
  error::{BatchedResult, IntoBatchedResult},
  module_loader::{module_loader::ModuleLoaderOutput, ModuleLoader},
  options::input_options::SharedInputOptions,
  plugin_driver::SharedPluginDriver,
  runtime::RuntimeModuleBrief,
  types::{
    module_table::ModuleTable, resolved_request_info::ResolvedRequestInfo, symbols::Symbols,
  },
  utils::resolve_id::resolve_id,
  SharedResolver,
};

pub struct ScanStage<Fs: FileSystem + Default> {
  input_options: SharedInputOptions,
  plugin_driver: SharedPluginDriver,
  fs: Fs,
  resolver: SharedResolver<Fs>,
}

#[derive(Debug)]
pub struct ScanStageOutput {
  pub module_table: ModuleTable,
  pub ast_table: IndexVec<NormalModuleId, OxcProgram>,
  pub entry_points: Vec<EntryPoint>,
  pub symbols: Symbols,
  pub runtime: RuntimeModuleBrief,
  pub warnings: Vec<BuildError>,
}

impl<Fs: FileSystem + Default + 'static> ScanStage<Fs> {
  pub fn new(
    input_options: SharedInputOptions,
    plugin_driver: SharedPluginDriver,
    fs: Fs,
    resolver: SharedResolver<Fs>,
  ) -> Self {
    Self { input_options, plugin_driver, fs, resolver }
  }

  #[tracing::instrument(skip_all)]
  pub async fn scan(&self) -> BatchedResult<ScanStageOutput> {
    tracing::info!("Start scan stage");
    assert!(!self.input_options.input.is_empty(), "You must supply options.input to rolldown");

    let mut module_loader = ModuleLoader::new(
      Arc::clone(&self.input_options),
      Arc::clone(&self.plugin_driver),
      self.fs.share(),
      Arc::clone(&self.resolver),
    );

    module_loader.try_spawn_runtime_module_task();

    let user_entries = self.resolve_user_defined_entries()?;

    let ModuleLoaderOutput { module_table, entry_points, symbols, runtime, warnings, ast_table } =
      module_loader.fetch_all_modules(user_entries).await?;

    tracing::debug!("Scan stage finished {module_table:#?}");

    Ok(ScanStageOutput { module_table, entry_points, symbols, runtime, warnings, ast_table })
  }

  /// Resolve `InputOptions.input`
  #[tracing::instrument(skip_all)]
  fn resolve_user_defined_entries(
    &self,
  ) -> BatchedResult<Vec<(Option<String>, ResolvedRequestInfo)>> {
    let resolver = &self.resolver;
    let plugin_driver = &self.plugin_driver;

    let resolved_ids =
      block_on_spawn_all(self.input_options.input.iter().map(|input_item| async move {
        let specifier = &input_item.import;
        match resolve_id(
          resolver,
          plugin_driver,
          specifier,
          None,
          HookResolveIdArgsOptions { is_entry: true, kind: ImportKind::Import },
          false,
        )
        .await
        {
          Ok(info) => {
            if info.is_external {
              return Err(BuildError::entry_cannot_be_external(info.path.path.as_str()));
            }
            Ok((input_item.name.clone(), info))
          }
          Err(e) => Err(e),
        }
      }));

    resolved_ids.into_batched_result()
  }
}
