/* STM32H723 内存布局
 *
 * 只用一个 RAM 区（AXI SRAM 前 320KB），理由：
 *   - 复位后 D1 域已上电，AXI SRAM 可访问，无需额外使能
 *   - DTCM 只有 128KB，装不下 8192 条样本（64KB）+ 两个任务栈
 *
 * 注意：此处刻意不放在 DTCM。DTCM 虽是 0 等待周期、时序更"干净"，
 * 但 8192 条双 u32 样本就要 64KB，加上任务栈会挤爆 128KB 上限。
 * 若后续要追求极致测量保真度，可改为：栈放 DTCM、样本缓冲放 AXI。
 *
 * 另：不在此处定义 _stack_start。cortex-m-rt 的 link.x 会 PROVIDE 它
 * （= RAM 末尾），重复定义会冲突。
 */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 1024K
  RAM   : ORIGIN = 0x24000000, LENGTH = 320K
}
