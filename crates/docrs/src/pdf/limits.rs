/// Budgets of one PDF export, on top of the package's [`crate::Limits`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PdfLimits {
    /// Decoded pixels in one image.
    pub max_image_pixels: u64,
    /// Decoded pixels of all the images of one document together.
    pub max_document_pixels: u64,
}

impl Default for PdfLimits {
    fn default() -> Self {
        PdfLimits {
            max_image_pixels: 32 << 20,
            max_document_pixels: 256 << 20,
        }
    }
}

impl PdfLimits {
    /// For a serverless function with 256 MB of memory, as
    /// [`crate::Limits::serverless`].
    pub fn serverless() -> Self {
        PdfLimits {
            max_image_pixels: 4 << 20,
            max_document_pixels: 32 << 20,
        }
    }

    /// No policy ceilings, for trusted input.
    pub fn trusted() -> Self {
        PdfLimits {
            max_image_pixels: u64::MAX,
            max_document_pixels: u64::MAX,
        }
    }
}
