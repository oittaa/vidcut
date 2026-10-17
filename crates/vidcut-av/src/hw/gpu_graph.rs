//! A filter graph of frames in GPU memory, freed downstream first.

use crate::error::Result;
use crate::ffmpeg::{self, ffi};

/// A filter graph of frames in GPU memory, freed downstream first: each
/// filter once the filters it feeds are gone. FFmpeg's Vulkan filters keep
/// the frames they last read until they are freed (vulkan.c
/// `ff_vk_exec_add_dep_frame`), but not their frames context, which the
/// source or the filter before them holds. FFmpeg frees a graph's filters
/// in the order they were made, sources first. Such a frame would then go
/// back to a freed frames context, and FFmpeg would free it through that
/// freed memory (`vulkan_frame_free`): a crash where another thread has
/// taken the memory (gotchas.md).
///
/// The inner graph stays here. Building and running go through these
/// methods, so a caller cannot take a `&mut` to that graph and replace it:
/// the replacement would drop the filters in the order they were made, and
/// [`Drop`] would then see an empty graph.
pub(crate) struct GpuGraph(ffmpeg::filter::Graph);

impl GpuGraph {
    /// A stream's graph, its slices on the shared pool ([`crate::pool::graph`]).
    pub(crate) fn new() -> Self {
        Self::for_reading(crate::decode::Reading::Stream)
    }

    pub(crate) fn for_reading(reading: crate::decode::Reading) -> Self {
        Self(crate::pool::graph(reading))
    }

    pub(crate) fn add(
        &mut self,
        filter: &ffmpeg::filter::Filter,
        name: &str,
        args: &str,
    ) -> std::result::Result<ffmpeg::filter::Context, ffmpeg::Error> {
        self.0.add(filter, name, args)
    }

    pub(crate) fn get(&mut self, name: &str) -> Option<ffmpeg::filter::Context> {
        self.0.get(name)
    }

    pub(crate) fn validate(&mut self) -> std::result::Result<(), ffmpeg::Error> {
        self.0.validate()
    }

    /// Parses `spec` between this graph's `in` and `out`.
    pub(crate) fn parse_filters(&mut self, spec: &str) -> Result<()> {
        crate::parts::parse_filters(&mut self.0, spec)
    }

    /// The graph pointer.
    ///
    /// # Safety
    /// Used only while this graph is alive. The graph is not freed through
    /// it: [`Drop`] frees the filters from the sink back, then the graph.
    pub(crate) unsafe fn as_mut_ptr(&mut self) -> *mut ffi::AVFilterGraph {
        unsafe { self.0.as_mut_ptr() }
    }

    /// The filter to free next: one that feeds no other, the last made of
    /// those.
    fn downstream_end(&mut self) -> Option<*mut ffi::AVFilterContext> {
        // SAFETY: the graph's own list of filters, and each filter's list
        // of its outputs, where FFmpeg clears the link to a freed filter.
        unsafe {
            let graph = self.0.as_mut_ptr();
            let count = (*graph).nb_filters as usize;
            if count == 0 {
                return None;
            }
            let filters = std::slice::from_raw_parts((*graph).filters, count);
            let feeds_none = |f: &*mut ffi::AVFilterContext| {
                let outputs = (**f).nb_outputs as usize;
                outputs == 0
                    || std::slice::from_raw_parts((**f).outputs, outputs)
                        .iter()
                        .all(|o| o.is_null())
            };
            Some(filters[filters.iter().rposition(feeds_none).unwrap_or(count - 1)])
        }
    }
}

impl Drop for GpuGraph {
    fn drop(&mut self) {
        while let Some(filter) = self.downstream_end() {
            // SAFETY: a filter of the graph, which leaves the graph's list.
            unsafe { ffi::avfilter_free(filter) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gpu_graph_frees_its_filters_from_the_sink_back() {
        let mut graph = GpuGraph::new();
        let find = |name| ffmpeg::filter::find(name).unwrap();
        let args = "video_size=64x64:pix_fmt=0:time_base=1/30:pixel_aspect=1/1";
        graph.add(&find("buffer"), "in", args).unwrap();
        graph.add(&find("buffersink"), "out", "").unwrap();
        graph.parse_filters("null").unwrap();
        let mut freed = Vec::new();
        while let Some(filter) = graph.downstream_end() {
            // SAFETY: a filter of the graph, freed once its kind is read.
            unsafe {
                let kind = std::ffi::CStr::from_ptr((*(*filter).filter).name);
                freed.push(kind.to_string_lossy().into_owned());
                ffi::avfilter_free(filter);
            }
        }
        assert_eq!(freed, ["buffersink", "null", "buffer"]);
    }
}
