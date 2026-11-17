use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    picture::{Picture, PictureType},
    tag::{Accessor, Tag, TagType},
};
use std::path::Path;

/// Probe only: callers supply a working copy, never the library original.
pub fn retag(path: &Path, cover: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut picture = Picture::from_reader(&mut std::io::Cursor::new(cover))?;
    picture.set_pic_type(PictureType::CoverFront);
    let mut audio = lofty::read_from_path(path)?;
    let kinds: &[TagType] = match audio.file_type() {
        FileType::Wav => &[TagType::RiffInfo, TagType::Id3v2],
        FileType::Mpeg => &[TagType::Id3v2],
        FileType::Flac => &[TagType::VorbisComments],
        _ => return Err("unsupported probe format".into()),
    };
    for &kind in kinds {
        if audio.tag(kind).is_none() {
            let mut tag = Tag::new(kind);
            if let Some(source) = audio.primary_tag() {
                if let Some(title) = source.title() {
                    tag.set_title(title.into_owned());
                }
                if let Some(comment) = source.comment() {
                    tag.set_comment(comment.into_owned());
                }
            }
            audio.insert_tag(tag);
        }
        let tag = audio.tag_mut(kind).unwrap();
        tag.set_artist("Тестовый исполнитель".into());
        tag.set_album("Тестовый альбом".into());
        if kind != TagType::RiffInfo {
            tag.remove_picture_type(PictureType::CoverFront);
            tag.push_picture(picture.clone());
        }
    }
    audio.save_to_path(path, WriteOptions::default())?;
    Ok(())
}
