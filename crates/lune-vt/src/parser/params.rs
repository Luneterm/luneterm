//! Port of xterm.js src/common/parser/Params.ts @ f447274.
//!
//! Params storage class. This type is used by the parser to accumulate sequence parameters and
//! sub parameters and transmit them to the input handler actions.
//!
//! NOTES (from upstream):
//!  - params object for action handlers is borrowed, use [`Params::to_array`] or `clone` to get
//!    a copy
//!  - never read beyond `params.length() - 1` (likely to contain arbitrary data)
//!  - [`Params::get_sub_params`] returns a borrowed slice, use [`Params::get_sub_params_all`]
//!    for cloned sub params
//!  - hardcoded limitations:
//!    - max. value for a single (sub) param is 2^31 - 1 (greater values are clamped to that)
//!    - max. 256 sub params possible
//!    - negative values are not allowed beside -1 (placeholder for default value)
//!
//! About ZDM (Zero Default Mode): ZDM is not orchestrated by this class. If the parser is in
//! ZDM, it should add 0 for empty params, otherwise -1. This does not apply to subparams, empty
//! subparams should always be added with -1.
//!
//! Rust notes (no behaviour change; see `docs/porting-xterm.md`):
//!  - JS typed arrays become `Vec`s sized once at construction. Reads and writes use `get` /
//!    `get_mut`, so an out-of-range index is a silent no-op or `None` (as in JS), never a panic.
//!    Values past `length` are stale but defined, exactly like the typed arrays.
//!  - `throw` becomes [`ParamsError`] (plan decision D5).
//!  - `_subParamsIdx` is a packed `Uint16Array` (`start << 8 | end`); it is a `u16` here too,
//!    so the wrap at 256 sub params behaves like upstream.

use std::fmt;

/// Max value supported for a single param/subparam (clamped to positive int32 range).
pub const MAX_VALUE: i32 = 0x7FFF_FFFF;
/// Max allowed subparams for a single sequence (hardcoded limitation).
const MAX_SUBPARAMS: usize = 256;

/// Default `maxLength` / `maxSubParamsLength` of upstream's constructor.
const DEFAULT_LIMIT: usize = 32;

/// What `Params` rejects where upstream throws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamsError {
    /// `addParam` / `addSubParam` got a value below -1.
    ValueTooSmall,
    /// `maxSubParamsLength` above 256.
    TooManySubParams,
}

impl fmt::Display for ParamsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParamsError::ValueTooSmall => "values lesser than -1 are not allowed",
            ParamsError::TooManySubParams => "maxSubParamsLength must not be greater than 256",
        })
    }
}

impl std::error::Error for ParamsError {}

/// One entry of upstream's `ParamsArray` (`(number | number[])[]`): a param or the sub params
/// of the param before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamsArrayItem {
    Param(i32),
    Sub(Vec<i32>),
}

/// Params storage. See the module docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    // params store and length
    params: Vec<i32>,
    length: usize,

    // sub params store and length
    sub_params: Vec<i32>,
    sub_params_length: usize,

    // sub params offsets from param: param idx --> [start, end] offset
    sub_params_idx: Vec<u16>,
    reject_digits: bool,
    reject_sub_digits: bool,
    digit_is_sub: bool,

    max_length: usize,
    max_sub_params_length: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self::new()
    }
}

impl Params {
    /// Create a `Params` with upstream's default limits (32 params, 32 sub params).
    pub fn new() -> Self {
        Self::with_storage(DEFAULT_LIMIT, DEFAULT_LIMIT)
    }

    /// `maxLength`: max length of storable parameters; `maxSubParamsLength`: max length of
    /// storable sub parameters (at most 256).
    pub fn with_limits(
        max_length: usize,
        max_sub_params_length: usize,
    ) -> Result<Self, ParamsError> {
        if max_sub_params_length > MAX_SUBPARAMS {
            return Err(ParamsError::TooManySubParams);
        }
        Ok(Self::with_storage(max_length, max_sub_params_length))
    }

    fn with_storage(max_length: usize, max_sub_params_length: usize) -> Self {
        Params {
            params: vec![0; max_length],
            length: 0,
            sub_params: vec![0; max_sub_params_length],
            sub_params_length: 0,
            sub_params_idx: vec![0; max_length],
            reject_digits: false,
            reject_sub_digits: false,
            digit_is_sub: false,
            max_length,
            max_sub_params_length,
        }
    }

    /// Create a `Params` from the JS array representation (default limits).
    pub fn from_array(values: &[ParamsArrayItem]) -> Result<Self, ParamsError> {
        let mut params = Self::new();
        // skip leading sub params
        let skip = usize::from(matches!(values.first(), Some(ParamsArrayItem::Sub(_))));
        for value in values.iter().skip(skip) {
            match value {
                ParamsArrayItem::Sub(subs) => {
                    for &sub in subs {
                        params.add_sub_param(sub)?;
                    }
                }
                ParamsArrayItem::Param(value) => params.add_param(*value)?,
            }
        }
        Ok(params)
    }

    /// Max length of storable parameters (`maxLength`).
    pub fn max_length(&self) -> usize {
        self.max_length
    }

    /// Max length of storable sub parameters (`maxSubParamsLength`).
    pub fn max_sub_params_length(&self) -> usize {
        self.max_sub_params_length
    }

    /// Number of params added since the last [`Params::reset`].
    pub fn length(&self) -> usize {
        self.length
    }

    /// The params added since the last reset (`params.params[0..length]`).
    pub fn values(&self) -> &[i32] {
        self.params.get(..self.length).unwrap_or(&[])
    }

    /// Raw param store at `idx`, like `params.params[idx]`: indices at or past `length()` hold
    /// stale data (upstream: "never read beyond `params.length - 1`"). `None` past the store.
    pub fn get(&self, idx: usize) -> Option<i32> {
        self.params.get(idx).copied()
    }

    /// Get a representation of the current parameters and sub parameters, structured as:
    ///
    /// ```text
    /// sequence: "1;2:3:4;5::6"
    /// array   : [1, 2, [3, 4], 5, [-1, 6]]
    /// ```
    pub fn to_array(&self) -> Vec<ParamsArrayItem> {
        let mut res = Vec::new();
        for i in 0..self.length {
            res.push(ParamsArrayItem::Param(self.get(i).unwrap_or(0)));
            if let Some(subs) = self.sub_slice(i) {
                res.push(ParamsArrayItem::Sub(subs.to_vec()));
            }
        }
        res
    }

    /// Reset to initial empty state.
    pub fn reset(&mut self) {
        self.length = 0;
        self.sub_params_length = 0;
        self.reject_digits = false;
        self.reject_sub_digits = false;
        self.digit_is_sub = false;
    }

    /// Add a parameter value.
    ///
    /// `Params` only stores up to `max_length` parameters, any later parameter will be ignored
    /// (and not validated, as upstream). Note: VT devices only stored up to 16 values, xterm
    /// seems to store up to 30.
    pub fn add_param(&mut self, value: i32) -> Result<(), ParamsError> {
        self.digit_is_sub = false;
        if self.length >= self.max_length {
            self.reject_digits = true;
            return Ok(());
        }
        if value < -1 {
            return Err(ParamsError::ValueTooSmall);
        }
        let packed = pack_idx(self.sub_params_length);
        if let Some(slot) = self.sub_params_idx.get_mut(self.length) {
            *slot = packed;
        }
        if let Some(slot) = self.params.get_mut(self.length) {
            // Upstream clamps `value > MAX_VALUE`; an `i32` cannot exceed it.
            *slot = value;
        }
        self.length += 1;
        Ok(())
    }

    /// Add a sub parameter value.
    ///
    /// The sub parameter is automatically associated with the last parameter value. Thus it is
    /// not possible to add a subparameter without any parameter added yet. `Params` only stores
    /// up to `max_sub_params_length` sub parameters, any later sub parameter will be ignored.
    pub fn add_sub_param(&mut self, value: i32) -> Result<(), ParamsError> {
        self.digit_is_sub = true;
        if self.length == 0 {
            return Ok(());
        }
        if self.reject_digits || self.sub_params_length >= self.max_sub_params_length {
            self.reject_sub_digits = true;
            return Ok(());
        }
        if value < -1 {
            return Err(ParamsError::ValueTooSmall);
        }
        if let Some(slot) = self.sub_params.get_mut(self.sub_params_length) {
            // Upstream clamps `value > MAX_VALUE`; an `i32` cannot exceed it.
            *slot = value;
        }
        self.sub_params_length += 1;
        // `Uint16Array` `++` wraps; so does this.
        if let Some(slot) = self.sub_params_idx.get_mut(self.length - 1) {
            *slot = slot.wrapping_add(1);
        }
        Ok(())
    }

    /// Whether parameter at index `idx` has sub parameters.
    pub fn has_sub_params(&self, idx: usize) -> bool {
        match self.sub_params_idx.get(idx) {
            Some(&packed) => (packed & 0xFF) > (packed >> 8),
            None => false,
        }
    }

    /// Return sub parameters for parameter at index `idx`.
    ///
    /// Note: the values are borrowed, thus you need to copy the values if you need to hold them
    /// in nonlocal scope.
    pub fn get_sub_params(&self, idx: usize) -> Option<&[i32]> {
        self.sub_slice(idx)
    }

    /// Return all sub parameters as `(idx, sub params)` pairs, in index order.
    ///
    /// Note: the values are not borrowed.
    pub fn get_sub_params_all(&self) -> Vec<(usize, Vec<i32>)> {
        let mut result = Vec::new();
        for i in 0..self.length {
            if let Some(subs) = self.sub_slice(i) {
                result.push((i, subs.to_vec()));
            }
        }
        result
    }

    /// Add a single digit value to current parameter.
    ///
    /// This is used by the parser to account digits on a char by char basis.
    pub fn add_digit(&mut self, value: i32) {
        let length = if self.digit_is_sub {
            self.sub_params_length
        } else {
            self.length
        };
        if self.reject_digits || length == 0 || (self.digit_is_sub && self.reject_sub_digits) {
            return;
        }

        let store = if self.digit_is_sub {
            &mut self.sub_params
        } else {
            &mut self.params
        };
        if let Some(slot) = store.get_mut(length - 1) {
            let cur = *slot;
            // `~cur` is truthy for every value but -1 (the "no value yet" placeholder). The
            // arithmetic is done in i64 so ten times an in-range value cannot overflow.
            *slot = if cur != -1 {
                (i64::from(cur) * 10 + i64::from(value)).min(i64::from(MAX_VALUE)) as i32
            } else {
                value
            };
        }
    }

    /// The `[start, end)` slice of the sub param store for param `idx`, if it has any.
    fn sub_slice(&self, idx: usize) -> Option<&[i32]> {
        let packed = *self.sub_params_idx.get(idx)?;
        let start = usize::from(packed >> 8);
        let end = usize::from(packed & 0xFF);
        if end > start {
            // `subarray` clamps to the store; so do we.
            let end = end.min(self.sub_params.len());
            Some(self.sub_params.get(start..end).unwrap_or(&[]))
        } else {
            None
        }
    }
}

/// `subParamsLength << 8 | subParamsLength` stored into a `Uint16Array` (truncating).
fn pack_idx(sub_params_length: usize) -> u16 {
    (sub_params_length << 8 | sub_params_length) as u16
}

#[cfg(test)]
#[path = "params_tests.rs"]
mod tests;
