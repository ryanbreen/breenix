
use crate::io::drivers::network::e1000::constants::*;

use crate::io::pci::DeviceError;

fn vlan_used() -> bool {
    /*
    // FIXME: I may eventually decide to support this.
    u16 vid;

    for_each_set_bit(vid, adapter->active_vlans, VLAN_N_VID)
        return true;
        */
    false
}

pub(in crate::io::drivers::network::e1000) fn toggle_vlan_filter(device: &super::E1000, filter_on:bool) -> Result<(), DeviceError<ErrorType>> {

    // FIXME: NET DEVICE SETUP
    //if (!test_bit(__E1000_DOWN, &adapter->flags))
    //    e1000_irq_disable(adapter);

    // __e1000_vlan_mode(adapter, adapter->netdev->features);
    if filter_on {
        /* enable VLAN receive filtering */
        let mut rctl = device.hardware.read(RCTL)?;
        rctl &= !RCTL_CFIEN;

        // FIXME: NET DEVICE SETUP
        /*
        if adapter->netdev->flags & IFF_PROMISC == 0 {
            rctl |= E1000_RCTL_VFE;
        }
        */

        device.hardware.write(RCTL, rctl)?;
        update_mng_vlan(device)?;
    } else {
        /* disable VLAN receive filtering */
        let mut rctl = device.hardware.read(RCTL)?;
        rctl &= !RCTL_VFE;
        device.hardware.write(RCTL, rctl)?;
    }

    // FIXME: NET DEVICE SETUP
    //if (!test_bit(__E1000_DOWN, &adapter->flags))
    //    e1000_irq_enable(adapter);

    Ok(())

}

pub(in crate::io::drivers::network::e1000) fn update_mng_vlan(device: &super::E1000) -> Result<(), DeviceError<ErrorType>> {

    /*
    let vid = self.hardware.mng_cookie.vlan_id;
    let old_vid = self.mng_vlan_id;

    if vlan_used() {
        return Ok(());
    }
    */

    // FIXME: I will eventually need to support this.
    /*
    if (!test_bit(vid, adapter->active_vlans)) {
        if (hw->mng_cookie.status &
            MNG_DHCP_COOKIE_STATUS_VLAN_SUPPORT) {
            vlan_rx_add_vid(netdev, htons(ETH_P_8021Q), vid);
            adapter->mng_vlan_id = vid;
        } else {
            adapter->mng_vlan_id = MNG_VLAN_NONE;
        }
        if ((old_vid != (u16)MNG_VLAN_NONE) &&
            (vid != old_vid) &&
            !test_bit(old_vid, adapter->active_vlans))
            vlan_rx_kill_vid(netdev, htons(ETH_P_8021Q),
                        old_vid);
    } else {
        adapter->mng_vlan_id = vid;
    }*/

    Ok(())
}