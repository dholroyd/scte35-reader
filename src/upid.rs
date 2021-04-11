//! _Unique Program Identifier_ types from a range of different identification schemes supported in
//! _SCTE-35_ `segmentation_descriptor()` messages.
//!
//! Instances of these types will be obtainable from the variants of the [`SegmentationUpid`](../enum.SegmentationUpid.html) enum found in the
//! [`SegmentationDescriptor::Insert.segmentation_upid`](../enum.SegmentationDescriptor.html#variant.Insert.field.segmentation_upid)
//! field.
//!
//!
//! The standard defines the following types of UPID, where Type and Length columns give values for `segmentation_upid_type` and `segmentation_upid_length` fields,
//!
//! | Type | Length (bytes) | Name | Description |
//! |------|----------------|------|-------------|
//! | `0x00` | `0` | _Not Used_ | The `segmentation_upid` is not defined and is not present in the descriptor. |
//! | `0x01` | _variable_ | _User defined_ | Deprecated: use type `0x0C`; The `segmentation_upid` does not follow a standard naming scheme. |
//! | `0x02` | `8` | ISCI | Deprecated: use type `0x03`, 8 characters; 4 alpha characters followed by 4 numbers. <br><br> e.g `ABCD1234` |
//! | `0x03` | `12` | Ad-ID | Defined by the Advertising Digital Identification, LLC group. 12 characters; 4 alpha characters (company identification prefix) followed by 8 alphanumeric characters. (See [Ad-ID](http://www.ad-id.org/)) <br><br> e.g. `ABCD0001000H` |
//! | `0x04` | `32` | UMID | See [SMPTE 330](https://en.wikipedia.org/wiki/Unique_Material_Identifier) <br><br> e.g. `060A2B34.01010105.​01010D20.13000000.​D2C9036C.8F195343.​AB7014D2.D718BFDA` |
//! | `0x05` | `8` | ISAN | Deprecated: use type `0x06`, ISO 15706 binary encoding. |
//! | `0x06` | `12` | ISAN | Formerly known as V-ISAN. ISO 15706-2 binary encoding (“versioned” ISAN). See [ISO15706-2](https://en.wikipedia.org/wiki/ISO_15706-2). <br><br> e.g. `0000-0001-2C52-0000-P-0000-0000-0` |
//! | `0x07` | `12` | TID | Tribune Media Systems Program identifier. 12 characters; 2 alpha characters followed by 10 numbers. <br><br> e.g. `MV0004146400` |
//! | `0x08` | `8` | TI | AiringID (Formerly Turner ID), used to indicate a specific airing of a program that is unique within a network. <br><br> e.g. `0x0A42235B81BC70FC` |
//! | `0x09` | _variable_ | ADI | CableLabs metadata identifier <br><br> e.g. `provider.com/MOVE1234567890123456` |
//! | `0x0A` | `12` | EIDR | An EIDR (see [EIDR](http://eidr.org/documents/EIDR_ID_Format_v1.3.pdfx)) represented in Compact Binary encoding <br><br> e.g. Content: `10.5240/0E4F-892E-442F-6BD4-15B0-1` Video Service: `10.5239/C370-DCA5` |
//! | `0x0B` | _variable_ | ATSC Content Identifier | `ATSC_content_identifier()` structure as defined in [ATSC A/57B](https://www.atsc.org/atsc-documents/a57b-content-identification-and-labeling-for-atsc-transport-revision-b/). |
//! | `0x0C` | _variable_ | `MPU()` | Managed Private UPID structure |
//! | `0x0D` | _variable_ | `MID()` | Multiple UPID types structure |
//! | `0x0E` | _variable_ | ADS Information | Advertising information. The specific usage is out of scope of this standard. |
//! | `0x0F` | _variable_ | URI | Universal Resource Identifier (see [RFC 3986](https://tools.ietf.org/html/rfc3986)). <br><br> e.g. `urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6` |
//! | `0x10` - `0xFF` | _variable_ | _Reserved_ | Reserved for future standardization. |

use hex_slice::AsHex;
use serde::Serializer;
use std::fmt;

fn hex_tuple(name: &str, f: &mut fmt::Formatter<'_>, val: &[u8]) -> fmt::Result {
    write!(f, "{}({:02x})", name, val.plain_hex(false))
}

/// Represents the UPID with type `0x01`, which the SCTE-35 standard says is deprecated.
#[derive(serde_derive::Serialize)]
pub struct UserDefinedDeprecated(pub Vec<u8>);
impl fmt::Debug for UserDefinedDeprecated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex_tuple("UserDefinedDeprecated", f, &self.0)
    }
}

/// _Industry Standard Commercial Identifier_
#[derive(Debug, serde_derive::Serialize)]
pub struct IsciDeprecated(pub String);

/// Defined by the _Advertising Digital Identification_ group
#[derive(Debug, serde_derive::Serialize)]
pub struct AdID(pub String);

/// Represents the UPID with type `0x05`, which the SCTE-35 standard says is deprecated.
#[derive(serde_derive::Serialize)]
pub struct IsanDeprecated(pub Vec<u8>);
impl fmt::Debug for IsanDeprecated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex_tuple("IsanDeprecated", f, &self.0)
    }
}

/// SMPTE ST 330:2011 Unique Material Identifier
#[derive(serde_derive::Serialize)]
pub struct Umid(pub Vec<u8>);
impl fmt::Debug for Umid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Umid(")?;
        for (i, c) in self.0.chunks(4).enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{:02x}", c.plain_hex(false))?;
        }
        f.write_str(")")
    }
}

/// Tribune Media Systems Program identifier
#[derive(Debug, serde_derive::Serialize)]
pub struct TID(pub String);

/// AiringID
///
/// (Formerly Turner ID)
#[derive(PartialEq, serde_derive::Serialize)]
pub struct TI(pub Vec<u8>);
impl fmt::Debug for TI {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex_tuple("TI", f, &self.0)
    }
}

/// Cablelabs metadata identifier
#[derive(Debug, serde_derive::Serialize)]
pub struct ADI(pub String);

/// An _Entertainment ID Registry Association_ identifier (compact binary representation)
#[derive(serde_derive::Serialize)]
pub struct EIDR(pub [u8; 12]);
impl fmt::Debug for EIDR {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex_tuple("EIDR", f, &self.0)
    }
}

/// `ATSC_content_identifier()` structure
#[derive(serde_derive::Serialize)]
pub struct ATSC(pub Vec<u8>);
impl fmt::Debug for ATSC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex_tuple("ATSC", f, &self.0)
    }
}

/// _Managed Private UPID_ structure
#[derive(serde_derive::Serialize)]
pub struct MPU(pub Vec<u8>);
impl fmt::Debug for MPU {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex_tuple("MPU", f, &self.0)
    }
}

/// _Advertising Information_ (SCTE-35 does not specify the format)
#[derive(Debug, serde_derive::Serialize)]
pub struct ADSInformation(pub Vec<u8>);

/// Just a wrapper around `url::Url` that adds serde serialisation
#[derive(Debug)]
pub struct Url(pub url::Url);
impl serde::Serialize for Url {
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use hex_literal::*;

    #[test]
    fn umid_fmt() {
        assert_eq!(
            "Umid(00000000.11111111.22222222.33333333.44444444.55555555.66666666.77889900)",
            format!(
                "{:?}",
                Umid(
                    hex!("0000000011111111222222223333333344444444555555556666666677889900")
                        .to_vec()
                )
            )
        )
    }
}
