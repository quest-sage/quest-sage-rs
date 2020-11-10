use std::mem::take;

use crate::graphics::*;
use futures::future::{BoxFuture, FutureExt};
use qs_common::assets::Asset;
use stretch::geometry::Point;

/// A `MultiBatch` renders layers of content by sending data to multiple batches
/// to be rendered with the fewest possible draw calls (within reasonable computational complexity).
///
/// To render something using a multibatch, it must be split into several layers, where the elements of
/// each layer are ideally rendered concurrently where possible.
pub struct MultiBatch {
    pub batch: Batch,
    pub text_renderer: TextRenderer,
}

struct MultiBatchRenderState<'a> {
    text_render_data: &'a mut Vec<(Point<f32>, RenderableWord)>,
    batch_render_data: &'a mut Vec<Renderable>,
    batch_render_texture: &'a mut Option<Asset<Texture>>,
    frame: &'a wgpu::SwapChainTexture,
    camera: &'a Camera,
}

impl MultiBatch {
    pub fn new(batch: Batch, text_renderer: TextRenderer) -> Self {
        Self {
            batch,
            text_renderer,
        }
    }

    /// The rendering algorithm essentially is that we should keep adding data to a list of
    /// text/batch items to render until we hit a new layer, after which we should render the intermediate
    /// lists to the batches.
    pub async fn render(
        &mut self,
        renderable: MultiRenderable,
        frame: &wgpu::SwapChainTexture,
        camera: &Camera,
        _profiler: qs_common::profile::ProfileSegmentGuard<'_>,
    ) {
        let mut text_render_data: Vec<(Point<f32>, RenderableWord)> = Vec::new();
        let mut batch_render_data: Vec<Renderable> = Vec::new();
        let mut batch_render_texture: Option<Asset<Texture>> = None;
        let mut state = MultiBatchRenderState {
            text_render_data: &mut text_render_data,
            batch_render_data: &mut batch_render_data,
            batch_render_texture: &mut batch_render_texture,
            frame,
            camera,
        };

        state.incremental_render(renderable, self).await;
        state.perform_render(self).await;
    }
}

impl<'a> MultiBatchRenderState<'a> {
    /// Appends render information to the given data, calling `perform_render` if we need to.
    fn incremental_render<'b>(
        &'b mut self,
        renderable: MultiRenderable,
        batch: &'b mut MultiBatch,
    ) -> BoxFuture<()> {
        async move {
            match renderable {
                MultiRenderable::Nothing => {}
                MultiRenderable::Layered(layers) => {
                    for (layer, index) in layers.into_iter().zip(0i32..) {
                        if index != 0 {
                            self.perform_render(batch).await;
                        }
                        self.incremental_render(layer, batch).await;
                    }
                }
                MultiRenderable::Adjacent(items) => {
                    for item in items {
                        self.incremental_render(item, batch).await;
                    }
                }
                MultiRenderable::Text { word, offset } => {
                    self.text_render_data.push((offset, word));
                }
                MultiRenderable::Image {
                    texture,
                    mut renderables,
                } => {
                    if let Some(tex) = self.batch_render_texture {
                        if tex.clone() != texture {
                            self.perform_render(batch).await;
                            *self.batch_render_texture = Some(texture);
                        }
                    } else {
                        *self.batch_render_texture = Some(texture);
                    }
                    self.batch_render_data.append(&mut renderables);
                }
            }
        }
        .boxed()
    }

    async fn perform_render<'b>(&'b mut self, batch: &'b mut MultiBatch) {
        if !self.text_render_data.is_empty() {
            batch.text_renderer.draw_text(
                take(self.text_render_data),
                self.frame,
                self.camera,
                //profiler.task("text").time(),
            );
        }
        if !self.batch_render_data.is_empty() {
            if let Some(tex) = self.batch_render_texture.take() {
                tex.if_loaded(|tex| {
                    batch.batch.render(
                        self.frame,
                        &tex,
                        self.camera,
                        take(self.batch_render_data).into_iter(),
                    );
                }).await;
            }
        }
    }
}

/// This contains high-level information about which batch to use for rendering, and how to configure it.
pub enum MultiRenderable {
    /// Render nothing.
    Nothing,

    /// This is a list of layers of renderables to render.
    /// - If this is empty, nothing will be rendered. No draw calls will be used.
    /// - If this has one element, that element will be rendered alongside sibling
    /// elements. The order of rendering is not guaranteed.
    /// - If this has more than one element, then previous layers are rendered before later layers.
    /// The whole element is rendered alongside sibling elements; the order of rendering between
    /// siblings is not guaranteed.
    Layered(Vec<MultiRenderable>),

    /// The list of items are rendered alongside each other with no regard for ordering.
    Adjacent(Vec<MultiRenderable>),

    /// Render some text using the text render batch.
    Text {
        word: RenderableWord,
        offset: Point<f32>,
    },

    /// Render a region (or multiple regions) of a texture using the regular batch.
    Image {
        texture: Asset<Texture>,
        renderables: Vec<Renderable>,
    },
}
