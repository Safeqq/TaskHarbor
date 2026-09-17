use taskharbor_core::{JobName, JobNameError};

fn main() -> Result<(), JobNameError> {
    let job_name = JobName::new("example-job")?;

    println!(
        "TaskHarbor API foundation is ready for job \"{}\".",
        job_name.as_str()
    );

    Ok(())
}
