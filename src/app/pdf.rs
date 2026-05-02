use std::sync::mpsc;

impl super::CkWriterApp {
    pub fn start_pdf_build(&mut self) {
        let Some(book) = &self.book else { return };
        if self.pdf_building {
            return;
        }
        self.pdf_error = None;
        self.pdf_building = true;
        self.pdf_meta = None;
        self.pdf_renderer = None;
        self.pdf_textures.clear();
        self.pdf_build_rx = Some(crate::pdf::build_and_meta(&book.root, self.pdf_dpi));
    }

    /// Open Read mode against an already-built PDF: just read metadata, don't
    /// rasterize anything up front. Pages render on demand from `pdf_view`.
    pub fn open_existing_pdf(&mut self) {
        let Some(book) = &self.book else { return };
        if self.pdf_building {
            return;
        }
        self.pdf_error = None;
        self.pdf_building = true;
        self.pdf_meta = None;
        self.pdf_renderer = None;
        self.pdf_textures.clear();
        self.pdf_build_rx = Some(crate::pdf::meta_only(&book.root, self.pdf_dpi));
    }

    pub(super) fn poll_pdf_build(&mut self) {
        let Some(rx) = &self.pdf_build_rx else { return };
        match rx.try_recv() {
            Ok(crate::pdf::BuildOutcome::Built(meta)) => {
                let book_root = self.book.as_ref().map(|b| b.root.clone());
                self.pdf_renderer = book_root
                    .map(|root| crate::pdf::PageRenderer::new(&root, meta.dpi, meta.page_count));
                self.pdf_meta = Some(meta);
                self.pdf_textures.clear();
                self.pdf_building = false;
                self.pdf_build_rx = None;
                self.pdf_error = None;
            }
            Ok(crate::pdf::BuildOutcome::Failed(msg)) => {
                self.pdf_error = Some(msg);
                self.pdf_building = false;
                self.pdf_build_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pdf_building = false;
                self.pdf_build_rx = None;
            }
        }
    }
}
