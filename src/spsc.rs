use std::alloc::{alloc, dealloc, Layout};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{marker::PhantomData, mem, ptr};

const CACHE_LINE_SIZE: usize = 64;

#[repr(C)]
pub struct Spsc<T> {
  pad0: [u8; CACHE_LINE_SIZE],
  size: usize,
  records: *mut T,
  read_index: AtomicUsize,
  pad1: [u8; CACHE_LINE_SIZE - mem::size_of::<AtomicUsize>()],
  write_index: AtomicUsize,
  pad2: [u8; CACHE_LINE_SIZE - mem::size_of::<AtomicUsize>()],
  _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Spsc<T> {}
unsafe impl<T: Send> Sync for Spsc<T> {}

impl<T> Spsc<T> {
  pub fn new(size: usize) -> Self {
    assert!(size >= 2, "size must be >= 2");
    let layout = Layout::array::<T>(size).expect("invalid layout");
    let records = unsafe { alloc(layout) as *mut T };
    if records.is_null() {
      panic!("allocation failed");
    }
    Spsc {
      pad0: [0; CACHE_LINE_SIZE],
      size,
      records,
      read_index: AtomicUsize::new(0),
      pad1: [0; CACHE_LINE_SIZE - mem::size_of::<AtomicUsize>()],
      write_index: AtomicUsize::new(0),
      pad2: [0; CACHE_LINE_SIZE - mem::size_of::<AtomicUsize>()],
      _marker: PhantomData,
    }
  }

  pub fn write(&self, record: T) -> bool {
    let current_write = self.write_index.load(Ordering::Relaxed);
    let next_record = (current_write + 1) % self.size;
    if next_record != self.read_index.load(Ordering::Acquire) {
      unsafe {
        ptr::write(self.records.add(current_write), record);
      }
      self.write_index.store(next_record, Ordering::Release);
      true
    } else {
      false
    }
  }

  // todo optimize
  pub fn write_all(&self, records: Vec<T>) {
    for i in records {
      self.write(i);
    }
  }

  pub fn read(&self) -> Option<T> {
    let current_read = self.read_index.load(Ordering::Relaxed);
    if current_read == self.write_index.load(Ordering::Acquire) {
      None
    } else {
      let next_record = (current_read + 1) % self.size;
      let record = unsafe { ptr::read(self.records.add(current_read)) };
      self.read_index.store(next_record, Ordering::Release);
      Some(record)
    }
  }

  // todo optimize
  pub fn read_all(&self) -> Vec<T> {
    let mut frames = Vec::with_capacity(32);
    while let Some(frame) = self.read() {
      frames.push(frame);
    }
    frames
  }

  pub fn front_ptr(&self) -> Option<&mut T> {
    let current_read = self.read_index.load(Ordering::Relaxed);
    if current_read == self.write_index.load(Ordering::Acquire) {
      None
    } else {
      unsafe { Some(&mut *self.records.add(current_read)) }
    }
  }

  pub fn pop_front(&self) {
    let current_read = self.read_index.load(Ordering::Relaxed);
    assert_ne!(
      current_read,
      self.write_index.load(Ordering::Acquire),
      "queue must not be empty"
    );
    let next_record = (current_read + 1) % self.size;
    unsafe {
      ptr::drop_in_place(self.records.add(current_read));
    }
    self.read_index.store(next_record, Ordering::Release);
  }

  pub fn is_empty(&self) -> bool {
    self.read_index.load(Ordering::Acquire) == self.write_index.load(Ordering::Acquire)
  }

  pub fn is_full(&self) -> bool {
    let next_record = (self.write_index.load(Ordering::Acquire) + 1) % self.size;
    next_record == self.read_index.load(Ordering::Acquire)
  }

  pub fn size_guess(&self) -> usize {
    let read_index = self.read_index.load(Ordering::Acquire);
    let write_index = self.write_index.load(Ordering::Acquire);
    if write_index >= read_index {
      write_index - read_index
    } else {
      self.size - read_index + write_index
    }
  }

  pub fn capacity(&self) -> usize {
    self.size - 1
  }
}

impl<T> Drop for Spsc<T> {
  fn drop(&mut self) {
    if !mem::needs_drop::<T>() {
      return;
    }

    let mut read_index = self.read_index.load(Ordering::Relaxed);
    let end_index = self.write_index.load(Ordering::Relaxed);
    while read_index != end_index {
      unsafe {
        ptr::drop_in_place(self.records.add(read_index));
      }
      read_index = (read_index + 1) % self.size;
    }

    unsafe {
      let layout = Layout::array::<T>(self.size).expect("invalid layout");
      dealloc(self.records as *mut u8, layout);
    }
  }
}
