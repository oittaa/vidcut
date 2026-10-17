use crate::error::Result;
use crate::ffmpeg::{self, ffi};

pub(crate) struct GpuGraph(ffmpeg::filter::Graph);

impl GpuGraph {
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

    pub(crate) fn parse_filters(&mut self, spec: &str) -> Result<()> {
        crate::parts::parse_filters(&mut self.0, spec)
    }

    pub(crate) unsafe fn as_mut_ptr(&mut self) -> *mut ffi::AVFilterGraph {
        unsafe { self.0.as_mut_ptr() }
    }

    fn downstream_end(&mut self) -> Option<*mut ffi::AVFilterContext> {
        // SAFETY: the graph lists `nb_filters` live filters, each with `nb_outputs` links or nulls.
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
