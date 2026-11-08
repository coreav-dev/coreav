use coreav_hevc::{DecodeSummary, PictureDecoder, native_rows::Admission};
use std::cell::RefCell;

#[derive(Default)]
pub struct Report {
    pub admissions: Vec<Admission>,
    pub summary: Option<DecodeSummary>,
    pub overflow: bool,
}

thread_local! {
    static REPORT: RefCell<Option<Report>> = const { RefCell::new(None) };
}

pub fn begin() {
    REPORT.with(|report| *report.borrow_mut() = Some(Report::default()));
}

pub fn take() -> Option<Report> {
    REPORT.with(|report| report.borrow_mut().take())
}

pub(crate) fn admitted(decoder: &PictureDecoder) {
    let Some(admission) = decoder.native_row_admission() else {
        return;
    };
    REPORT.with(|report| {
        let mut report = report.borrow_mut();
        let Some(report) = report.as_mut() else {
            return;
        };
        if report.admissions.last() == Some(&admission) {
            return;
        }
        if report.admissions.len() == 1024 || report.admissions.try_reserve(1).is_err() {
            report.overflow = true;
            return;
        }
        report.admissions.push(admission);
    });
}

pub(crate) fn completed(summary: DecodeSummary) {
    REPORT.with(|report| {
        if let Some(report) = report.borrow_mut().as_mut() {
            report.summary = Some(summary);
        }
    });
}
