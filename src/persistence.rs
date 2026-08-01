use crate::model::ProjectV1;
use std::{fs::{self,File,OpenOptions},io::{self,BufWriter,Write},path::{Path,PathBuf}};

#[derive(Debug,thiserror::Error)]
pub enum ProjectIoError {
    #[error("could not read {path}: {source}")] Read { path:PathBuf,#[source] source:io::Error },
    #[error("invalid project JSON in {path}: {source}")] Json { path:PathBuf,#[source] source:serde_json::Error },
    #[error("invalid project in {path}: {source}")] Validation { path:PathBuf,#[source] source:crate::model::ValidationError },
    #[error("could not save {path}: {source}")] Save { path:PathBuf,#[source] source:io::Error },
}

pub fn load(path:&Path)->Result<ProjectV1,ProjectIoError>{
    let bytes=fs::read(path).map_err(|source|ProjectIoError::Read{path:path.into(),source})?;
    let project:ProjectV1=serde_json::from_slice(&bytes).map_err(|source|ProjectIoError::Json{path:path.into(),source})?;
    project.validate().map_err(|source|ProjectIoError::Validation{path:path.into(),source})?;
    Ok(project)
}

pub fn save_atomic(path:&Path,project:&ProjectV1)->Result<(),ProjectIoError>{
    project.validate().map_err(|source|ProjectIoError::Validation{path:path.into(),source})?;
    let parent=path.parent().unwrap_or_else(||Path::new("."));
    let name=path.file_name().and_then(|n|n.to_str()).unwrap_or("project");
    let tmp=parent.join(format!(".{name}.{}.tmp",std::process::id()));
    let result=(||->io::Result<()> {
        let file=OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let mut out=BufWriter::new(file);
        serde_json::to_writer_pretty(&mut out,project).map_err(io::Error::other)?;
        out.write_all(b"\n")?; out.flush()?; out.get_ref().sync_all()?;
        fs::rename(&tmp,path)?;
        if let Ok(dir)=File::open(parent){let _=dir.sync_all();}
        Ok(())
    })();
    if result.is_err(){let _=fs::remove_file(&tmp);}
    result.map_err(|source|ProjectIoError::Save{path:path.into(),source})
}

#[cfg(test)] mod tests {
 use super::*;
 #[test] fn round_trip_and_newline(){let d=tempfile::tempdir().unwrap();let f=d.path().join("x.groove.json");let p=ProjectV1::new();save_atomic(&f,&p).unwrap();assert_eq!(load(&f).unwrap(),p);assert!(fs::read(&f).unwrap().ends_with(b"\n"));}
 #[test] fn reject_unknown(){let d=tempfile::tempdir().unwrap();let f=d.path().join("x");fs::write(&f,r#"{"format_version":1,"globals":{},"tracks":[],"wat":1}"#).unwrap();assert!(load(&f).is_err());}
 #[test] fn default_schema_uses_required_names(){let value=serde_json::to_value(ProjectV1::new()).unwrap();assert_eq!(value["format_version"],1);assert_eq!(value["globals"]["key"],"C");assert_eq!(value["globals"]["delay_division"],"eighth");assert_eq!(value["tracks"].as_array().unwrap().len(),6);assert_eq!(value["tracks"][0]["name"],"Kick");assert!(value["tracks"][0].get("input_degree").is_none());}
}
