//! Icons: GPUI Kit's default set plus the Lucide icons the Files page uses.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    Extra,
    [
        Braces,
        ChevronsDownUp,
        Copy,
        FileCode,
        FileCog,
        FileDigit,
        FileImage,
        FileKey,
        FileLock,
        FileQuestionMark,
        FileSymlink,
        FileTerminal,
        FileX,
        FolderSearch,
        FolderTree,
        House,
        Lock,
        Pencil,
        Save,
        Unlink,
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match Extra.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(Extra.list(path)?);
        Ok(paths)
    }
}
