use super::*;

#[test]
fn notes_do_not_count_toward_the_wall_blob_limit() {
    let dir = tempfile::tempdir().unwrap();
    git::plant(dir.path()).unwrap();
    let mut importer = Importer::new(dir.path(), &git::read(dir.path()).unwrap()).unwrap();
    let file = crate::seed::seed().unwrap().to_file();
    let mut files = crate::tree::to_files(&file).unwrap();
    files.insert(
        "notes/1/proof.md".into(),
        vec![b'x'; usize::try_from(crate::store::MAX_WALL_FILE).unwrap() + 1],
    );
    let commit = Commit::whole("start", 0, None, files, &[]);
    let mark = importer.commit(&commit).unwrap();
    importer.checkpoint(mark).unwrap();
    importer.finish().unwrap();
    let read = super::super::files(dir.path(), "HEAD").unwrap();
    assert_eq!(read, crate::tree::to_files(&file).unwrap());
}
