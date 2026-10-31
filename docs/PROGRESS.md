# OneAPI 项目约定

本文档只记录需要跨会话遵守的项目约定。进度、已完成事项与版本变更以 Git 提交和 Release 记录为准，不在本文档重复。

## 文档入口

- `AGENTS.md`：AOCI 仓库认知规则与项目文档入口。
- `docs/PROGRESS.md`：本文件，项目约定（文档、分支、发布、交接与认知维护）。
- `docs/KNOWN_LIMITATIONS.md`：当前版本仍存在的已知限制与接受的风险。
- `README.md`：功能、部署、环境变量与在线更新信任模型。
- 架构、数据模型与协议映射以代码和 AOCI 认知索引为准。

## 分支与发布约定

- `develop`：长期开发与集成分支，日常功能、修复和文档变更都在此进行；默认从 `develop` 切出工作分支。
- `feature/*`、`fix/*`、`security/*`：短期工作分支，从 `develop` 切出，完成后合回 `develop` 并删除；命名沿用 `feature/<主题>-<版本>`、`fix/<主题>`、`security/<主题>-<版本>`。
- `main`：发布基线，只合入已验证的 `develop` 内容；不直接在 `main` 上开发。
- `release/*`：发布前冻结或补丁需要时从 `develop` 临时切出，发布后合回 `main`，并按需回合并到 `develop` 后删除。
- 版本发布：`main` 上的 `Cargo.toml` 版本与 `v*` Tag 必须一致，Tag 触发 Release 流水线；发布后在 `develop` 同步版本号与文档。
- 已合并分支应及时删除（本地与远端），历史由 Tag 与 `main` 保留。

## 文档与变更约定

- 不维护里程碑、进度、变更记录或已完成事项文档；这些信息以 Git 提交与 Release 为准。
- 约定变化时更新本文档；已知限制新增或解除时更新 `docs/KNOWN_LIMITATIONS.md`。
- 架构或关键决策变化时直接修改对应代码与注释，并同步 AOCI 认知条目，不另建计划文档。
- 审查/优化项完成后不写回文档，提交信息与代码即记录。

## AOCI 认知维护约定

- 受管理对象达到最终稳定状态后调用一次 `aoci_maintain`，按机器签发的完整批次更新认知条目。
- `aoci.txt`、`aoci.meta.txt`、`aoci.code.txt`、`.aoci/config.json`、`.aoci/baseline.json`、`AGENTS.md`、`.gitattributes` 属于应纳入版本管理的认知资产；其余 `.aoci` 运行时资产不提交。
- 提交时把 AOCI 托管资产与业务改动分开记录。

## 交接约定

- 新会话开始先读 `AGENTS.md`、本文档和 `docs/KNOWN_LIMITATIONS.md`。
- 提交前保证 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 与前端 `npm run lint`、`npm run build` 通过。
