/// Budgets of one PDF export, on top of the package's [`crate::Limits`]
/// (final.md §10). Pixel counts are in millions (M = 1,000,000), byte counts
/// in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PdfLimits {
    /// Pages in one PDF.
    pub max_pages: u32,
    /// Bytes of the PDF file, enforced while writing.
    pub max_output_bytes: u64,
    /// Decoded pixels in one image.
    pub max_image_pixels: u64,
    /// Decoded pixels of all the images of one document together.
    pub max_document_pixels: u64,
}

impl Default for PdfLimits {
    fn default() -> Self {
        PdfLimits {
            max_pages: 2_000,
            max_output_bytes: 256 << 20,
            max_image_pixels: 32_000_000,
            max_document_pixels: 256_000_000,
        }
    }
}

impl PdfLimits {
    /// For a serverless function with 256 MB of memory, as
    /// [`crate::Limits::serverless`].
    pub fn serverless() -> Self {
        PdfLimits {
            max_pages: 500,
            max_output_bytes: 32 << 20,
            max_image_pixels: 4_000_000,
            max_document_pixels: 32_000_000,
        }
    }

    /// No policy ceilings for trusted input, beyond what PDF's integers and
    /// a 4 GiB file allow.
    pub fn trusted() -> Self {
        PdfLimits {
            max_pages: u32::MAX,
            max_output_bytes: 4 << 30,
            max_image_pixels: u64::MAX,
            max_document_pixels: u64::MAX,
        }
    }
}
