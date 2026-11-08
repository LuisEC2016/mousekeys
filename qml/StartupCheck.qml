import QtQuick
import qs.Common
import qs.Services

QtObject {
    function check() {
        if (CompositorService.isNiri)
            return null;
        return {
            "title": I18n.trFor("niriMouseKeys", "Niri Mouse Keys only works on niri"),
            "details": I18n.trFor("niriMouseKeys", "It drives the pointer through niri's zwlr_virtual_pointer_v1 and niri key binds.")
        };
    }
}
