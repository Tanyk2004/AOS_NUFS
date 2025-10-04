# 0) HARD CLEAN (removes any leftover state)
sudo tc qdisc del dev "$IFACE" root    2>/dev/null || true
sudo tc qdisc del dev "$IFACE" ingress 2>/dev/null || true
sudo tc qdisc del dev ifb0 root        2>/dev/null || true
sudo ip link set ifb0 down             2>/dev/null || true
sudo ip link del ifb0                  2>/dev/null || true

# 1) EGRESS: prio root + netem child
sudo tc qdisc add dev "$IFACE" root handle 1: prio bands 3
sudo tc qdisc add dev "$IFACE" parent 1:3 handle 30: netem delay "$DELAY" 5ms distribution normal

# 2) EGRESS filters (match only SERVER + selected ports)
sudo tc filter add dev "$IFACE" parent 1: protocol ip flower dst_ip "$SERVER" ip_proto tcp dst_port 2049 classid 1:3
sudo tc filter add dev "$IFACE" parent 1: protocol ip flower dst_ip "$SERVER" ip_proto tcp dst_port 111  classid 1:3
sudo tc filter add dev "$IFACE" parent 1: protocol ip flower dst_ip "$SERVER" ip_proto tcp dst_port 20048 classid 1:3
sudo tc filter add dev "$IFACE" parent 1: protocol ip flower dst_ip "$SERVER" ip_proto tcp dst_port 22   classid 1:3

# 3) INGRESS path via IFB (to delay both directions)
sudo modprobe ifb numifbs=1 || true
sudo ip link add ifb0 type ifb 2>/dev/null || true
sudo ip link set ifb0 up
sudo tc qdisc add dev "$IFACE" handle ffff: ingress

# Redirect only replies from SERVER for those ports into ifb0
for SPORT in 2049 111 20048 22; do
  sudo tc filter add dev "$IFACE" parent ffff: protocol ip flower src_ip "$SERVER" ip_proto tcp src_port $SPORT \
    action mirred egress redirect dev ifb0
done

# Apply the same delay on the IFB side
sudo tc qdisc add dev ifb0 root netem delay "$DELAY" 5ms distribution normal
