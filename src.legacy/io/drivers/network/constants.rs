/*
 *	IEEE 802.3 Ethernet magic constants.  The frame sizes omit the preamble
 *	and FCS/CRC (frame check sequence).
 */

pub(in crate::io::drivers::network) const ETH_ALEN: u32 = 6; /* Octets in one ethernet addr	 */
pub(in crate::io::drivers::network) const ETH_HLEN: u32 = 14; /* Total octets in header.	 */
pub(in crate::io::drivers::network) const ETH_ZLEN: u32 = 60; /* Min. octets in frame sans FCS */
pub(in crate::io::drivers::network) const ETH_DATA_LEN: u32 = 1500; /* Max. octets in payload	 */
pub(in crate::io::drivers::network) const ETH_FRAME_LEN: u32 = 1514; /* Max. octets in frame sans FCS */
pub(in crate::io::drivers::network) const ETH_FCS_LEN: u32 = 4; /* Octets in the FCS		 */

pub const MAXIMUM_ETHERNET_VLAN_SIZE: u32 = 1522;
