use std::fmt::Display;

#[derive(Default, Clone)]
pub struct OptimizeResult {
    pub total_chunks: usize,
    pub deleted_chunks: usize,
    pub deleted_regions: usize,
    pub io_errors: usize,
    pub compression_failures: usize,
    pub regions_with_compression_issues: usize,
    pub header_write_failures: usize,
    pub regions_with_header_issues: usize,
}

impl Display for OptimizeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Optimization Result:\n\
                   Total Chunks: {}\n\
                   Deleted Chunks: {}\n\
                   Deleted Regions: {}\n\
                   I/O Errors: {}\n\
                   Compression Fallbacks: {}\n\
                   Regions With Compression Issues: {}\n\
                   Header Write Failures: {}\n\
                   Regions With Header Issues: {}",
            self.total_chunks,
            self.deleted_chunks,
            self.deleted_regions,
            self.io_errors,
            self.compression_failures,
            self.regions_with_compression_issues,
            self.header_write_failures,
            self.regions_with_header_issues
        )
    }
}

pub fn reduce_optimize_results(results: &mut [OptimizeResult]) -> OptimizeResult {
    results
        .iter_mut()
        .reduce(|acc, cur| {
            acc.deleted_regions += cur.deleted_regions;
            acc.total_chunks += cur.total_chunks;
            acc.deleted_chunks += cur.deleted_chunks;
            acc.io_errors += cur.io_errors;
            acc.compression_failures += cur.compression_failures;
            acc.regions_with_compression_issues += cur.regions_with_compression_issues;
            acc.header_write_failures += cur.header_write_failures;
            acc.regions_with_header_issues += cur.regions_with_header_issues;
            acc
        })
        .cloned()
        .unwrap_or_default()
}
