use core::any::Any;

/// 设备块 Trait
/// 以块为单位读写数据
pub trait BlockDevice: Send + Sync + Any {
    fn read_block(&self, block_id: usize, buf: &mut [u8]);
    fn write_block(&self, block_id: usize, buf: &[u8]);
    
    /// TODO: 返回设备 ID
    fn get_id(&self) -> usize {
        114514
    }
}
