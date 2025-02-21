use http::header::{HeaderMap, HeaderName, HeaderValue};

#[derive(Debug)]
pub struct VersionInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub rustc: RustcInfo,
    pub build: BuildInfo,
    pub git: GitInfo,
}

impl VersionInfo {
    pub const NAME: HeaderName = HeaderName::from_static("name");
    pub const VERSION: HeaderName = HeaderName::from_static("version");

    pub fn as_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();

        headers.insert(Self::NAME, HeaderValue::from_static(self.name));
        headers.insert(Self::VERSION, HeaderValue::from_static(self.version));
        headers.extend(self.rustc.as_headers().drain());
        headers.extend(self.build.as_headers().drain());
        headers.extend(self.git.as_headers().drain());

        headers
    }
}

#[derive(Debug)]
pub struct RustcInfo {
    pub version: &'static str,
    pub commit_date: &'static str,
    pub commit_hash: &'static str,
}

impl RustcInfo {
    pub const VERSION: HeaderName = HeaderName::from_static("rustc-version");
    pub const COMMIT_DATE: HeaderName = HeaderName::from_static("rustc-commit-date");
    pub const COMMIT_HASH: HeaderName = HeaderName::from_static("rustc-commit-hash");

    fn as_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();

        headers.insert(Self::VERSION, HeaderValue::from_static(self.version));
        headers.insert(
            Self::COMMIT_DATE,
            HeaderValue::from_static(self.commit_date),
        );
        headers.insert(
            Self::COMMIT_HASH,
            HeaderValue::from_static(self.commit_hash),
        );

        headers
    }
}

#[derive(Debug)]
pub struct BuildInfo {
    pub target: &'static str,
    pub debug: bool,
    pub opt_level: &'static str,
    pub timestamp: &'static str,
}

impl BuildInfo {
    pub const TARGET: HeaderName = HeaderName::from_static("build-target");
    pub const DEBUG: HeaderName = HeaderName::from_static("build-debug");
    pub const OPT_LEVEL: HeaderName = HeaderName::from_static("build-opt-level");
    pub const TIMESTAMP: HeaderName = HeaderName::from_static("build-timestamp");

    fn as_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();

        headers.insert(Self::TARGET, HeaderValue::from_static(self.target));
        headers.insert(Self::DEBUG, HeaderValue::from_static(unbool(self.debug)));
        headers.insert(Self::OPT_LEVEL, HeaderValue::from_static(self.opt_level));
        headers.insert(Self::TIMESTAMP, HeaderValue::from_static(self.timestamp));

        headers
    }
}

#[derive(Debug)]
pub struct GitInfo {
    pub branch: &'static str,
    pub commit: &'static str,
    pub is_dirty: bool,
}

impl GitInfo {
    pub const BRANCH: HeaderName = HeaderName::from_static("git-branch");
    pub const COMMIT: HeaderName = HeaderName::from_static("git-commit");
    pub const IS_DIRTY: HeaderName = HeaderName::from_static("git-is-dirty");

    fn as_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();

        headers.insert(Self::BRANCH, HeaderValue::from_static(self.branch));
        headers.insert(Self::COMMIT, HeaderValue::from_static(self.commit));
        headers.insert(
            Self::IS_DIRTY,
            HeaderValue::from_static(unbool(self.is_dirty)),
        );

        headers
    }
}

fn unbool(x: bool) -> &'static str {
    if x { "true" } else { "false" }
}

pub const VERSION_INFO: VersionInfo = VersionInfo {
    name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
    rustc: RustcInfo {
        version: env!("VERGEN_RUSTC_SEMVER"),
        commit_date: env!("VERGEN_RUSTC_COMMIT_DATE"),
        commit_hash: env!("VERGEN_RUSTC_COMMIT_HASH"),
    },
    build: BuildInfo {
        target: env!("VERGEN_CARGO_TARGET_TRIPLE"),
        debug: const_str::parse!(env!("VERGEN_CARGO_DEBUG"), bool),
        opt_level: env!("VERGEN_CARGO_OPT_LEVEL"),
        timestamp: env!("VERGEN_BUILD_TIMESTAMP"),
    },
    git: GitInfo {
        branch: env!("VERGEN_GIT_BRANCH"),
        commit: env!("VERGEN_GIT_SHA"),
        is_dirty: const_str::parse!(env!("VERGEN_GIT_DIRTY"), bool),
    },
};
