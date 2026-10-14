//! Transfer errors. Messages are shown to the user, so they are Chinese and
//! name at most a file, never its contents.

use std::io;

use osm_protocol::v1::{TransferErrorCode, TransferFailed};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("拖拽的项目已失效，请重新拖拽")]
    NotFound,
    #[error("没有权限读取“{0}”")]
    PermissionDenied(String),
    #[error("“{0}”在传输过程中被修改，请重新拖拽")]
    SourceChanged(String),
    #[error("项目过多或文件夹层级过深（{0}）")]
    Limit(String),
    #[error("磁盘空间不足：需要 {needed} 字节，可用 {available} 字节")]
    DiskFull { needed: u64, available: u64 },
    #[error("目标位置已有同名项目“{0}”")]
    NameConflict(String),
    #[error("“{0}”校验失败，数据可能已损坏")]
    ChecksumMismatch(String),
    #[error("对方发送的文件清单无效：{0}")]
    InvalidManifest(String),
    #[error("传输协议错误：{0}")]
    Protocol(String),
    /// Reported by the other side; the message is already user-facing.
    #[error("{0}")]
    Remote(String),
    #[error("已取消")]
    Cancelled,
    #[error("读写失败：{0}")]
    Io(#[from] io::Error),
}

impl TransferError {
    pub(crate) fn to_wire(&self) -> TransferFailed {
        let code = match self {
            Self::NotFound => TransferErrorCode::NotFound,
            Self::PermissionDenied(_) => TransferErrorCode::PermissionDenied,
            Self::SourceChanged(_) => TransferErrorCode::SourceChanged,
            Self::Limit(_) => TransferErrorCode::Limit,
            _ => TransferErrorCode::Io,
        };
        TransferFailed {
            code: code as i32,
            message: self.to_string(),
        }
    }

    pub(crate) fn from_wire(f: TransferFailed) -> Self {
        match TransferErrorCode::try_from(f.code) {
            Ok(TransferErrorCode::NotFound) => Self::NotFound,
            _ if f.message.is_empty() => Self::Remote("对方读取文件失败".into()),
            _ => Self::Remote(f.message.chars().take(512).collect()),
        }
    }
}
